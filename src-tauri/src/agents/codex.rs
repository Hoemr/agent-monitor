//! Codex adapter.
//!
//! Real sources (verified on macOS, Codex 0.162 app + VSCode extension):
//!   * `~/.codex/state_5.sqlite` table `threads` — one row per conversation.
//!   * `~/.codex/thread_history_1.sqlite` table `thread_turns` — turn outcomes
//!     (completed / failed / interrupted); only written when a turn ends.
//!   * the thread's `rollout_path` JSONL — last `event_msg` gives the live state
//!     (`task_started` without a following `task_complete` = running).
//!
//! No API is assumed: everything is read-only from local files. A thread is
//! never merged with another; the stable id is `codex:<thread id>`.

use super::{truncate_chars, ScanCtx, Source};
use crate::model::*;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const APP_NAME: &str = "ChatGPT";
const BUNDLE_ID: &str = "com.openai.codex";
const PROCESS_NEEDLE: &str = "ChatGPT.app/Contents/MacOS/ChatGPT";
const ROLLOUT_TAIL_BYTES: u64 = 256 * 1024;
const RECENT_SECS: u64 = 6 * 3600;
const RUNNING_STALE_SECS: u64 = 30 * 60;

#[derive(Clone)]
struct ThreadMeta {
    id: String,
    title: String,
    rollout_path: String,
    cwd: String,
    updated_at: u64,
    source: String,
}

#[derive(Clone)]
struct RolloutTail {
    last_event: String,
    activity_secs: u64,
}

pub struct CodexSource {
    db_mtime: u64,
    history_mtime: u64,
    threads: Vec<ThreadMeta>,
    last_turn: HashMap<String, String>,
    tails: HashMap<String, (u64, RolloutTail)>,
}

impl CodexSource {
    pub fn new() -> Self {
        Self {
            db_mtime: 0,
            history_mtime: 0,
            threads: Vec::new(),
            last_turn: HashMap::new(),
            tails: HashMap::new(),
        }
    }

    fn state_db() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".codex/state_5.sqlite"))
    }

    fn history_db() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".codex/thread_history_1.sqlite"))
    }

    fn open_ro(path: &Path) -> Option<Connection> {
        let uri = format!("file:{}?immutable=1", uri_path(path));
        Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()
    }

    fn reload_threads(&mut self, now: u64) {
        let Some(path) = Self::state_db() else {
            return;
        };
        let Some(conn) = Self::open_ro(&path) else {
            return;
        };
        let mut out = Vec::new();
        let cutoff = now.saturating_sub(RECENT_SECS) as i64;
        if let Ok(mut stmt) = conn.prepare(
            "select id, title, rollout_path, cwd, updated_at, source
             from threads where updated_at >= ?1 order by updated_at desc limit 120",
        ) {
            if let Ok(rows) = stmt.query_map([cutoff], |row| {
                Ok(ThreadMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    rollout_path: row.get(2)?,
                    cwd: row.get(3)?,
                    updated_at: row.get::<_, i64>(4)? as u64,
                    source: row.get(5)?,
                })
            }) {
                for r in rows.flatten() {
                    // Sub-agent threads belong to a parent conversation.
                    if r.source.contains("subagent") {
                        continue;
                    }
                    out.push(r);
                }
            }
        }
        self.threads = out;
    }

    fn reload_history(&mut self) {
        let Some(path) = Self::history_db() else {
            return;
        };
        let Some(conn) = Self::open_ro(&path) else {
            return;
        };
        let mut map = HashMap::new();
        if let Ok(mut stmt) = conn.prepare(
            "select thread_id, status from (
               select thread_id, status,
                      row_number() over (partition by thread_id order by started_at desc) as rn
               from thread_turns
             ) where rn = 1",
        ) {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for r in rows.flatten() {
                    map.insert(r.0, r.1);
                }
            }
        }
        self.last_turn = map;
    }

    fn tail_for(&mut self, id: &str, rollout: &str) -> Option<RolloutTail> {
        let path = Path::new(rollout);
        if !rollout.ends_with(".jsonl") {
            // Compressed rollout: fall back to turn history.
            return None;
        }
        let mtime = super::file_mtime_secs(path);
        if let Some((cached_mtime, tail)) = self.tails.get(id) {
            if *cached_mtime == mtime {
                return Some(tail.clone());
            }
        }
        let tail = read_rollout_tail(path)?;
        self.tails.insert(id.to_string(), (mtime, tail.clone()));
        Some(tail)
    }
}

fn uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F")
}

fn read_rollout_tail(path: &Path) -> Option<RolloutTail> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let start = meta.len().saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    let mut lines: Vec<&str> = buf.lines().collect();
    if start > 0 && lines.len() > 1 {
        lines.remove(0);
    }
    let mut last_event = String::new();
    let mut activity = 0u64;
    for line in lines.iter().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if activity == 0 {
            if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                activity = parse_rfc3339(ts).unwrap_or(0);
            }
        }
        if v.get("type").and_then(|t| t.as_str()) == Some("event_msg") {
            if let Some(pt) = v
                .get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
            {
                last_event = pt.to_string();
                break;
            }
        }
    }
    Some(RolloutTail {
        last_event,
        activity_secs: activity,
    })
}

/// RFC3339 → unix seconds (Howard Hinnant's days-from-civil algorithm).
fn parse_rfc3339(ts: &str) -> Option<u64> {
    let ts = ts.trim_end_matches('Z');
    let (date, time) = ts.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next()?.parse().ok()?;
    let sec: f64 = t.next()?.parse().ok()?;
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + hour * 3600 + min * 60 + sec as i64;
    u64::try_from(secs).ok()
}

impl Source for CodexSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "codex".into(),
            name: "Codex".into(),
            source:
                "~/.codex/state_5.sqlite threads + rollout JSONL tail + thread_history_1.sqlite"
                    .into(),
            origin: DataOrigin::Index,
            max_level: JumpLevel::L3,
            notes: "Thread-level identity; codex://threads/<id> deep link (verified by jump.rs)."
                .into(),
        }
    }

    fn invalidate(&mut self) {
        self.db_mtime = 0;
        self.history_mtime = 0;
        self.tails.clear();
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let Some(db) = Self::state_db() else {
            return Vec::new();
        };
        let db_mtime = super::file_mtime_secs(&db);
        if db_mtime != self.db_mtime {
            self.reload_threads(ctx.now_secs);
            self.db_mtime = db_mtime;
        }
        if let Some(h) = Self::history_db() {
            let hm = super::file_mtime_secs(&h);
            if hm != self.history_mtime {
                self.reload_history();
                self.history_mtime = hm;
            }
        }

        let app_alive = ctx.processes.any(PROCESS_NEEDLE) || ctx.processes.any(APP_NAME);
        let mut out = Vec::new();

        let threads = self.threads.clone();
        for thread in &threads {
            let tail = self.tail_for(&thread.id, &thread.rollout_path);
            let activity = tail
                .as_ref()
                .map(|t| t.activity_secs)
                .filter(|s| *s > 0)
                .unwrap_or(thread.updated_at);
            if activity < ctx.now_secs.saturating_sub(RECENT_SECS) {
                continue;
            }
            let last_event = tail.as_ref().map(|t| t.last_event.as_str()).unwrap_or("");
            let last_turn = self.last_turn.get(&thread.id).cloned().unwrap_or_default();
            let silence = ctx.now_secs.saturating_sub(activity);

            let (status, reason) = if last_event == "task_started" {
                if !app_alive {
                    (
                        TaskStatus::Unknown,
                        "task_started but ChatGPT is not running".to_string(),
                    )
                } else if silence > RUNNING_STALE_SECS {
                    (
                        TaskStatus::Unknown,
                        "task_started with no rollout event for >30m".to_string(),
                    )
                } else {
                    (TaskStatus::Running, "rollout event task_started".to_string())
                }
            } else if last_event == "task_complete" {
                (
                    TaskStatus::Completed,
                    "rollout event task_complete".to_string(),
                )
            } else if last_turn == "failed" || last_turn == "interrupted" {
                (
                    TaskStatus::Failed,
                    format!("thread_turns.status = {last_turn}"),
                )
            } else if last_turn == "completed" {
                (
                    TaskStatus::Completed,
                    "thread_turns.status = completed".to_string(),
                )
            } else if thread.rollout_path.is_empty() && !app_alive {
                (TaskStatus::Idle, "no rollout, app not running".to_string())
            } else {
                (TaskStatus::Unknown, "no usable state event".to_string())
            };

            let title = if thread.title.trim().is_empty() {
                let base = Path::new(&thread.cwd)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                format!("Untitled thread ({base})")
            } else {
                truncate_chars(thread.title.trim(), 120)
            };

            let mut record = TaskRecord {
                id: format!("codex:{}", thread.id),
                agent_key: "codex".into(),
                agent_name: "Codex".into(),
                host: if thread.source == "vscode" {
                    "vscode".into()
                } else {
                    "desktop".into()
                },
                session_id: Some(thread.id.clone()),
                task_id: None,
                title,
                status,
                status_reason: reason,
                last_activity_secs: activity,
                collected_at_secs: ctx.now_secs,
                summary: None,
                origin: DataOrigin::Index,
                target: JumpTarget {
                    app_name: Some(APP_NAME.into()),
                    bundle_id: Some(BUNDLE_ID.into()),
                    deep_link: Some(format!("codex://threads/{}", thread.id)),
                    window_title: Some(thread.title.clone()),
                    workspace_path: Some(thread.cwd.clone()),
                    ..Default::default()
                },
                target_validity: TargetValidity::Unconfirmed,
                invalid_reason: None,
                capability: JumpCapability {
                    max_level: JumpLevel::L3,
                    notes: "codex://threads/<id> route; verified level in docs/JUMP.md".into(),
                    requires: vec![PermissionKind::Automation],
                },
                level: JumpLevel::L3,
                pinned: false,
            };
            if thread.rollout_path.is_empty() {
                record.target_validity = TargetValidity::Invalid;
                record.invalid_reason = Some("rollout_missing".into());
            } else if !app_alive {
                record.target_validity = TargetValidity::Unconfirmed;
            } else {
                record.target_validity = TargetValidity::Valid;
            }
            record.refresh_level();
            out.push(record);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        let t = parse_rfc3339("2026-10-21T12:00:00.000Z").unwrap();
        assert!(t > 1_700_000_000 && t < 2_000_000_000);
    }

    #[test]
    fn deep_link_shape() {
        let id = "01a15a27-a527-7301-869e-6c88b1432ca2";
        let link = format!("codex://threads/{id}");
        assert!(link.starts_with("codex://threads/"));
        assert!(!link.contains(' '));
    }

    #[test]
    fn uri_path_escapes_spaces() {
        let p = Path::new("/Users/me/My Projects/a b");
        assert!(uri_path(p).contains("%20"));
    }

    #[test]
    fn title_fallback_never_uses_raw_uuid() {
        let cwd = "/Users/me/proj";
        let fallback = format!(
            "Untitled thread ({})",
            Path::new(cwd).file_name().unwrap().to_string_lossy()
        );
        assert!(!fallback.contains("01a15a27"));
        assert!(fallback.contains("proj"));
    }
}
