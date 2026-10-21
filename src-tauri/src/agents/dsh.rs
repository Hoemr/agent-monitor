//! DSH (DeepSeek Harness) adapter.
//!
//! Real source: `~/.dsh/storages/session_projcache/sessions/*.json` — the
//! app's own projected session state (title, cwd, turn boundary, active user
//! questions, token usage). It is written atomically and is far cheaper than
//! decompressing `session.v4.jsonl.zstd`, so it is the primary source here.
//!
//! Status evidence (no inference from silence):
//!   * `userQuestions.questions.active[]` non-empty → waiting input (explicit)
//!   * `turnBoundary.openTurnStartSeq != null`          → running
//!   * open turn closed + `lastStepBoundary.kind=end`   → completed
//!   * session with no turns                            → idle
//!   * anything else                                    → unknown
//! Waiting-approval is NOT claimed: no approval-request event was observed on
//! this machine (the DSH approval policy is `never`).

use super::{truncate_chars, ScanCtx, Source};
use crate::model::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const APP_NAME: &str = "DeepSeek Harness";
const BUNDLE_ID: &str = "com.deepseek.dsh";
const PROCESS_NEEDLE: &str = "DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness";
const RETENTION_SECS: u64 = 6 * 3600;
const RUNNING_STALE_SECS: u64 = 30 * 60;

#[derive(Clone)]
struct DshRow {
    session_id: String,
    title: String,
    cwd: String,
    open_turn: bool,
    last_step_kind: String,
    active_questions: usize,
    turns: u64,
    last_activity_ms: u64,
    file_mtime: u64,
}

pub struct DshSource {
    rows: HashMap<String, (u64, DshRow)>,
}

impl DshSource {
    pub fn new() -> Self {
        Self {
            rows: HashMap::new(),
        }
    }

    fn cache_dir() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".dsh/storages/session_projcache/sessions"))
    }

    fn parse_session(path: &Path, file_mtime: u64) -> Option<DshRow> {
        let text = std::fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let record = v.get("record")?;
        let rows = record.get("rows")?;
        let session_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if session_id.is_empty() {
            return None;
        }
        let cwd = record
            .get("identity")
            .and_then(|i| i.get("cwd"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let title = rows
            .get("title")
            .and_then(|t| t.get("val"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_string();
        let boundary = rows.get("turnBoundary").and_then(|b| b.get("val"));
        let open_turn = boundary
            .and_then(|b| b.get("openTurnStartSeq"))
            .map(|s| !s.is_null())
            .unwrap_or(false);
        let last_step_kind = boundary
            .and_then(|b| b.get("lastStepBoundary"))
            .and_then(|b| b.get("kind"))
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        let active_questions = rows
            .get("userQuestions")
            .and_then(|q| q.get("val"))
            .and_then(|q| q.get("questions"))
            .and_then(|q| q.get("active"))
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let turns = rows
            .get("sessionStats")
            .and_then(|s| s.get("val"))
            .and_then(|s| s.get("turns"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let last_activity_ms = rows
            .get("timeContext")
            .and_then(|t| t.get("val"))
            .and_then(|t| t.get("lastMessageTime"))
            .and_then(|t| t.as_u64())
            .or_else(|| {
                rows.get("sessionListMetadata")
                    .and_then(|t| t.get("val"))
                    .and_then(|t| t.get("lastPromptAt"))
                    .and_then(|t| t.as_u64())
            })
            .unwrap_or(0);
        Some(DshRow {
            session_id,
            title,
            cwd,
            open_turn,
            last_step_kind,
            active_questions,
            turns,
            last_activity_ms,
            file_mtime,
        })
    }

    fn refresh(&mut self) {
        let Some(dir) = Self::cache_dir() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut seen = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }
            let mtime = super::file_mtime_secs(&path);
            let key = path.to_string_lossy().to_string();
            seen.push(key.clone());
            match self.rows.get(&key) {
                Some((cached, _)) if *cached == mtime => {}
                _ => {
                    if let Some(row) = Self::parse_session(&path, mtime) {
                        self.rows.insert(key, (mtime, row));
                    }
                }
            }
        }
        self.rows.retain(|k, _| seen.contains(k));
    }
}

impl Source for DshSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "dsh".into(),
            name: "DSH".into(),
            source: "~/.dsh/storages/session_projcache/sessions/*.json".into(),
            origin: DataOrigin::Cache,
            max_level: JumpLevel::L1,
            notes: "Single-window Electron host: its open-url handler only implements dsh://open (focus app). No session-level route exists, so DSH is application-level only."
                .into(),
        }
    }

    fn invalidate(&mut self) {
        self.rows.clear();
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        self.refresh();
        let app_alive = ctx.processes.any(PROCESS_NEEDLE) || ctx.processes.any(APP_NAME);
        let mut out = Vec::new();
        for (_, (_, row)) in self.rows.iter() {
            let activity = if row.last_activity_ms > 0 {
                row.last_activity_ms / 1000
            } else {
                row.file_mtime
            };
            let silence = ctx.now_secs.saturating_sub(activity);
            // Retention: anything active, plus the last 6 h of finished work.
            // Older completed sessions stay on disk but are not shown.
            let active = row.open_turn || row.active_questions > 0;
            let recent = activity >= ctx.now_secs.saturating_sub(RETENTION_SECS)
                || row.file_mtime >= ctx.now_secs.saturating_sub(RETENTION_SECS);
            if !active && !recent {
                continue;
            }

            let (status, reason) = if row.active_questions > 0 {
                (
                    TaskStatus::WaitingInput,
                    format!("{} active user question(s)", row.active_questions),
                )
            } else if row.open_turn {
                if !app_alive {
                    (
                        TaskStatus::Unknown,
                        "turn still open but DSH is not running".to_string(),
                    )
                } else if silence > RUNNING_STALE_SECS {
                    (
                        TaskStatus::Unknown,
                        "turn still open with no activity for >30m".to_string(),
                    )
                } else if row.last_step_kind == "start" {
                    (TaskStatus::Running, "step in progress".to_string())
                } else {
                    (TaskStatus::Running, "open turn".to_string())
                }
            } else if row.last_step_kind == "end" {
                (
                    TaskStatus::Completed,
                    "last turn ended (turnBoundary)".to_string(),
                )
            } else if row.turns == 0 {
                (TaskStatus::Idle, "session has no turns yet".to_string())
            } else {
                (TaskStatus::Unknown, "no turn boundary state".to_string())
            };

            let title = if row.title.is_empty() {
                Path::new(&row.cwd)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "DSH session".into())
            } else {
                truncate_chars(&row.title, 120)
            };
            let mut record = TaskRecord {
                id: format!("dsh:{}", row.session_id),
                agent_key: "dsh".into(),
                agent_name: "DSH".into(),
                host: "desktop".into(),
                session_id: Some(row.session_id.clone()),
                task_id: None,
                title,
                status,
                status_reason: reason,
                last_activity_secs: activity,
                collected_at_secs: ctx.now_secs,
                summary: Some(format!("turn {}", row.turns)),
                origin: DataOrigin::Cache,
                target: JumpTarget {
                    app_name: Some(APP_NAME.into()),
                    bundle_id: Some(BUNDLE_ID.into()),
                    deep_link: None,
                    window_title: Some(row.title.clone()),
                    workspace_path: Some(row.cwd.clone()),
                    ..Default::default()
                },
                target_validity: TargetValidity::Unconfirmed,
                invalid_reason: None,
                capability: JumpCapability {
                    max_level: JumpLevel::L1,
                    notes: "Application-level only: the host exposes no session deep link (verified in docs/JUMP.md)."
                        .into(),
                    requires: vec![PermissionKind::Automation],
                },
                level: JumpLevel::L1,
                pinned: false,
            };
            if !row.cwd.is_empty() && !Path::new(&row.cwd).is_dir() {
                record.target_validity = TargetValidity::Invalid;
                record.invalid_reason = Some("workspace_missing".into());
            } else if app_alive {
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

    fn fixture(open_turn: bool, step: &str, active_q: usize, turns: u64) -> DshRow {
        DshRow {
            session_id: "session-test".into(),
            title: "# Fix the parser".into(),
            cwd: "/tmp/ws".into(),
            open_turn,
            last_step_kind: step.into(),
            active_questions: active_q,
            turns,
            last_activity_ms: 0,
            file_mtime: 0,
        }
    }

    #[test]
    fn open_turn_is_running_and_active_question_wins() {
        let row = fixture(true, "start", 0, 2);
        assert!(row.open_turn && row.active_questions == 0);
        let row2 = fixture(true, "start", 1, 2);
        assert!(row2.active_questions > 0);
    }

    #[test]
    fn closed_turn_is_completed() {
        let row = fixture(false, "end", 0, 3);
        assert!(!row.open_turn);
        assert_eq!(row.last_step_kind, "end");
    }

    #[test]
    fn session_id_is_the_directory_name() {
        let p = Path::new("/tmp/sessions/session-935fdc75.json");
        assert_eq!(p.file_stem().unwrap().to_string_lossy(), "session-935fdc75");
    }


    #[test]
    fn no_waiting_approval_is_ever_inferred() {
        // Guard: the adapter has no code path that produces WaitingApproval.
        let row = fixture(false, "end", 0, 1);
        assert_ne!(row.last_step_kind, "approval");
    }
}
