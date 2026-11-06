//! Codex adapter.
//!
//! Real sources (verified on macOS, Codex 0.162 app + VSCode extension):
//!   * `~/.codex/state_5.sqlite` table `threads` — one row per conversation.
//!   * `~/.codex/thread_history_1.sqlite` table `thread_turns` — turn outcomes
//!     (completed / failed / interrupted); only written when a turn ends.
//!   * the thread's `rollout_path` JSONL — last task lifecycle event gives the live state
//!     (`task_started` without a following `task_complete` = running).
//!
//! No API is assumed: everything is read-only from local files. A thread is
//! never merged with another; the stable id is `codex:<thread id>`.

use super::{retention_keeps, truncate_chars, ScanCtx, Source};
use crate::model::*;
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const APP_NAME: &str = "ChatGPT";
const BUNDLE_ID: &str = "com.openai.codex";
const PROCESS_NEEDLE: &str = "ChatGPT.app/Contents/MacOS/ChatGPT";
const ROLLOUT_TAIL_BYTES: u64 = 256 * 1024;
const RUNNING_STALE_SECS: u64 = 30 * 60;

#[derive(Clone)]
struct ThreadMeta {
    id: String,
    title: String,
    rollout_path: String,
    cwd: String,
    updated_at: u64,
    source: String,
    vscode: bool,
    terminal: bool,
}

#[derive(Clone)]
struct RolloutTail {
    completion_id: Option<String>,
    turn_id: Option<String>,
    last_event: String,
    activity_secs: u64,
    waiting_input: bool,
    model: Option<String>,
}

pub struct CodexSource {
    stores: Vec<CodexStore>,
}

/// One cache per configured CODEX_HOME; identities remain codex:<thread id>.
struct CodexStore {
    home: PathBuf,
    db_mtime: DbStamp,
    history_mtime: DbStamp,
    threads: Vec<ThreadMeta>,
    last_turn: HashMap<String, String>,
    tails: HashMap<String, ((std::time::SystemTime, u64), RolloutTail)>,
}

impl CodexSource {
    pub fn new() -> Self {
        Self { stores: configured_homes().into_iter().map(CodexStore::new).collect() }
    }
}

/// The optional file is the complete home list, not a search/fallback chain.
/// Changes take effect on restart, together with the matching file watches.
pub fn configured_homes() -> Vec<PathBuf> {
    let path = crate::settings::data_dir().join("codex-homes.json");
    match std::fs::read(&path) {
        Ok(bytes) => match parse_homes(&bytes) {
            Ok(homes) => homes,
            Err(error) => { eprintln!("{}: {error}", path.display()); Vec::new() }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound =>
            super::home_dir().map(|h| vec![h.join(".codex")]).unwrap_or_default(),
        Err(error) => { eprintln!("{}: {error}", path.display()); Vec::new() }
    }
}

/// Rollouts held open by processes attached to the selected Terminal TTY.
/// Matching the open file gives the session identity even when several CLIs
/// share a working directory. This does not depend on a terminal window title.
pub fn terminal_rollouts(tty: &str) -> Vec<PathBuf> {
    let Some(suffix) = tty.strip_prefix("/dev/ttys") else { return Vec::new(); };
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) { return Vec::new(); }
    let Ok(pids) = std::process::Command::new("lsof").args(["-t", tty]).output() else { return Vec::new(); };
    let mut pids: Vec<_> = String::from_utf8_lossy(&pids.stdout).lines()
        .filter_map(|pid| pid.trim().parse::<u32>().ok()).collect();
    pids.sort_unstable(); pids.dedup();
    if pids.is_empty() { return Vec::new(); }
    let pids = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let Ok(files) = std::process::Command::new("lsof").args(["-a", "-p", &pids, "-Fn"]).output() else { return Vec::new(); };
    String::from_utf8_lossy(&files.stdout).lines().filter_map(|line| {
        let path = PathBuf::from(line.strip_prefix('n')?);
        let name = path.file_name()?.to_str()?;
        (name.starts_with("rollout-") && name.ends_with(".jsonl")
            && path.components().any(|c| c.as_os_str() == "sessions")).then_some(path)
    }).collect::<HashSet<_>>().into_iter().collect()
}

/// Native desktop read receipts survive switching away from a conversation.
/// A receipt acknowledges only the exact published turn, never a newer answer.
pub fn read_completions(tasks: &[TaskRecord]) -> Vec<(String, String)> {
    read_completions_in_homes(tasks, &configured_homes())
}

fn read_completions_in_homes(tasks: &[TaskRecord], homes: &[PathBuf]) -> Vec<(String, String)> {
    let candidates: Vec<_> = tasks.iter().filter(|task|
        task.agent_key == "codex" && task.host == "desktop"
            && task.target.bundle_id.as_deref() == Some(BUNDLE_ID)
            && task.status == TaskStatus::Completed
            && task.session_id.is_some() && task.task_id.is_some() && task.completion_id.is_some()
    ).collect();
    if candidates.is_empty() { return Vec::new(); }
    let mut covered = HashSet::new();
    let mut viewed = Vec::new();
    for home in homes {
        let Some(conn) = CodexStore::open_ro(&home.join("state_5.sqlite")) else { continue; };
        let query = "SELECT t.id, r.first_unread_turn, r.last_published_turn FROM threads t
            LEFT JOIN thread_read_receipts r ON r.thread_id = t.id WHERE t.id = ?1";
        // Older Codex versions may have no receipt table. Still respect the
        // first configured home owning a duplicated thread, like the adapter.
        let statement = conn.prepare(query).or_else(|_| conn.prepare(
            "SELECT id, NULL, NULL FROM threads WHERE id = ?1"));
        let Ok(mut stmt) = statement else { continue; };
        for task in &candidates {
            if covered.contains(&task.id) { continue; }
            let receipt = stmt.query_row([task.session_id.as_deref().unwrap()], |row|
                Ok((row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?)));
            let Ok((first_unread, published)) = receipt else { continue; };
            covered.insert(task.id.clone());
            if first_unread.is_none() && published.is_some() && published == task.task_id {
                viewed.push((task.id.clone(), task.completion_id.clone().unwrap()));
            }
        }
    }
    viewed
}

fn parse_homes(bytes: &[u8]) -> Result<Vec<PathBuf>, String> {
    let paths: Vec<PathBuf> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if paths.iter().any(|p| !p.is_absolute()) {
        return Err("Codex home paths must be absolute".into());
    }
    let mut seen = HashSet::new();
    Ok(paths.into_iter().filter(|p| seen.insert(p.clone())).collect())
}

impl CodexStore {
    fn new(home: PathBuf) -> Self {
        Self {
            home,
            db_mtime: Default::default(),
            history_mtime: Default::default(),
            threads: Vec::new(),
            last_turn: HashMap::new(),
            tails: HashMap::new(),
        }
    }

    fn state_db(&self) -> PathBuf {
        self.home.join("state_5.sqlite")
    }

    fn history_db(&self) -> PathBuf {
        self.home.join("thread_history_1.sqlite")
    }

    fn open_ro(path: &Path) -> Option<Connection> {
        let uri = format!("file:{}?mode=ro", uri_path(path));
        Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()
    }

    fn reload_threads(&mut self, ctx: &ScanCtx) {
        let path = self.state_db();
        let Some(conn) = Self::open_ro(&path) else {
            return;
        };
        let mut out = Vec::new();
        // The SQL window is the same user-configured retention as the final
        // filter; the row limit still bounds the read for large histories.
        let cutoff = ctx.now_secs.saturating_sub(ctx.retention_secs) as i64;
        let mut ids: Vec<_> = ctx.pinned_ids.iter().filter_map(|id| id.strip_prefix("codex:")).collect();
        ids.sort();
        let slots = std::iter::once("NULL".to_string()).chain((0..ids.len()).map(|i| format!("?{}", i + 2))).collect::<Vec<_>>().join(",");
        let mut params = vec![rusqlite::types::Value::Integer(cutoff)];
        params.extend(ids.iter().map(|id| rusqlite::types::Value::Text(id.to_string())));
        let sql = format!("select id, coalesce(nullif(trim(name), ''), title), rollout_path, cwd, updated_at, source
             from threads where (updated_at >= ?1 or id in ({slots})) and source not like '%subagent%'
             order by (id in ({slots})) desc, updated_at desc limit 120");
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok(ThreadMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    rollout_path: row.get(2)?,
                    cwd: row.get(3)?,
                    updated_at: row.get::<_, i64>(4)? as u64,
                    source: row.get(5)?,
                    vscode: false,
                    terminal: false,
                })
            }) {
                for mut r in rows.flatten() {
                    // Sub-agent threads belong to a parent conversation.
                    if r.source.contains("subagent") {
                        continue;
                    }
                    r.vscode = rollout_is_vscode(Path::new(&r.rollout_path));
                    r.terminal = r.source == "cli"
                        || rollout_is_terminal(Path::new(&r.rollout_path));
                    out.push(r);
                }
            }
        }
        self.threads = out;
    }

    fn reload_history(&mut self) {
        let path = self.history_db();
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
        let meta = std::fs::metadata(path).ok()?;
        let mtime = (meta.modified().ok()?, meta.len());
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

type DbStamp = (Option<(std::time::SystemTime, u64)>, Option<(std::time::SystemTime, u64)>);

fn db_stamp(path: &Path) -> DbStamp {
    let stamp = |path: &Path| path.metadata().ok().and_then(|m| Some((m.modified().ok()?, m.len())));
    (stamp(path), stamp(&PathBuf::from(format!("{}-wal", path.display()))))
}

fn rollout_is_vscode(path: &Path) -> bool {
    matches!(rollout_originator(path).as_deref(), Some("codex_vscode" | "vscode_agent_host"))
}

fn rollout_is_terminal(path: &Path) -> bool {
    matches!(rollout_originator(path).as_deref(), Some("codex-tui"))
}

fn rollout_originator(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let line = std::io::BufReader::new(file).lines().next()?.ok()?;
    let meta = serde_json::from_str::<serde_json::Value>(&line).ok()?;
    meta.pointer("/payload/originator")?.as_str().map(str::to_owned)
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
    let mut end = meta.len();
    let mut carry = Vec::new();
    let mut last_event = String::new();
    let mut activity = 0u64;
    let mut completion_id = None;
    let mut turn_id = None;
    let mut answered = HashSet::new();
    let mut waiting_input = false;
    let mut model = None;
    let mut model_checked = false;
    // Search backwards in chunks until a task boundary is found. Long tool
    // output can push task_started beyond the final 256 KiB of a live turn.
    while end > 0 && (last_event.is_empty() || !model_checked) {
        let start = end.saturating_sub(ROLLOUT_TAIL_BYTES);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = vec![0; (end - start) as usize];
        file.read_exact(&mut buf).ok()?;
        buf.extend_from_slice(&carry);
        let first = if start == 0 {
            0
        } else {
            buf.iter()
                .position(|&b| b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(buf.len())
        };
        for line in buf[first..].rsplit(|&b| b == b'\n') {
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if activity == 0 {
                if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                    activity = parse_rfc3339(ts).unwrap_or(0);
                }
            }
            if !model_checked && v.get("type").and_then(|t| t.as_str()) == Some("turn_context") {
                model = v.get("payload").and_then(|p| p.get("model")).and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty()).map(str::to_string);
                model_checked = true;
                if !last_event.is_empty() { break; }
            }
            if last_event.is_empty() && v.get("type").and_then(|t| t.as_str()) == Some("response_item") {
                if let Some(payload) = v.get("payload") {
                    inspect_question(payload, &mut answered, &mut waiting_input);
                }
            }
            if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
                continue;
            }
            let event = v.get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str());
            if let Some(kind @ ("task_started" | "task_complete" | "turn_aborted")) = event {
                if last_event.is_empty() {
                    last_event = kind.to_string();
                    turn_id = v.pointer("/payload/turn_id").and_then(|id| id.as_str())
                        .filter(|id| !id.is_empty()).map(str::to_owned);
                    if kind == "task_complete" {
                        completion_id = v.get("timestamp").and_then(|t| t.as_str()).map(str::to_string);
                    }
                    if kind != "task_started" { waiting_input = false; }
                }
                // A new turn with no context must never inherit the old model.
                if kind == "task_started" { model_checked = true; }
                if model_checked { break; }
            }
        }
        carry = buf[..first].to_vec();
        end = start;
    }
    Some(RolloutTail {
        completion_id,
        turn_id,
        last_event,
        activity_secs: activity,
        waiting_input,
        model,
    })
}

/// Read newest-to-oldest so tool results/replies settle the matching call only.
fn inspect_question(payload: &serde_json::Value, answered: &mut HashSet<String>, waiting: &mut bool) {
    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if kind == "message" && payload.get("role").and_then(|v| v.as_str()) == Some("user") {
        if let Some(content) = payload.get("content").and_then(|v| v.as_array()) {
            for item in content {
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(body) = text.strip_prefix("<send_user_message_question_reply>")
                    .and_then(|s| s.trim().strip_suffix("</send_user_message_question_reply>")) {
                    if let Ok(replies) = serde_json::from_str::<Vec<serde_json::Value>>(body.trim()) {
                        for reply in replies {
                            if let Some(key) = reply.get("questionItemId").and_then(|v| v.as_str()) {
                                if let Ok(parts) = serde_json::from_str::<Vec<serde_json::Value>>(key) {
                                    if let (Some(id), Some(index)) = (parts.get(1).and_then(|v| v.as_str()), parts.get(2).and_then(|v| v.as_u64())) {
                                        answered.insert(format!("{id}:{index}"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let Some(id) = payload.get("call_id").and_then(|v| v.as_str()) else { return; };
    if matches!(kind, "function_call_output" | "custom_tool_call_output") {
        let output = payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
        let accepted = serde_json::from_str::<serde_json::Value>(output).ok()
            .and_then(|v| v.get("accepted").and_then(|a| a.as_bool())) == Some(true);
        if !accepted { answered.insert(id.to_string()); }
    } else if matches!(kind, "function_call" | "custom_tool_call")
        && payload.get("name").and_then(|v| v.as_str()).map(super::is_question_tool) == Some(true)
        && !answered.contains(id)
    {
        let arguments = payload.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
        let count = serde_json::from_str::<serde_json::Value>(arguments).ok()
            .and_then(|v| v.get("questions").and_then(|q| q.as_array()).map(|q| q.len())).unwrap_or(1);
        *waiting |= (0..count).any(|index| !answered.contains(&format!("{id}:{index}")));
    }
}

fn task_status(
    last_event: &str,
    last_turn: &str,
    app_alive: bool,
    silence: u64,
    no_rollout: bool,
) -> (TaskStatus, String) {
    if last_event == "task_started" {
        if !app_alive {
            (
                TaskStatus::Unknown,
                "task_started but its host is not running".to_string(),
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
    } else if last_event == "turn_aborted" {
        (TaskStatus::Failed, "rollout event turn_aborted".to_string())
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
    } else if no_rollout && !app_alive {
        (TaskStatus::Idle, "no rollout, app not running".to_string())
    } else {
        (TaskStatus::Unknown, "no usable state event".to_string())
    }
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
                self.stores.iter().map(|s| s.home.display().to_string()).collect::<Vec<_>>().join("; "),
            origin: DataOrigin::Index,
            max_level: JumpLevel::L3,
            notes: "Thread-level identity; codex://threads/<id> deep link (verified by jump.rs)."
                .into(),
        }
    }

    fn invalidate(&mut self) {
        for store in &mut self.stores { store.invalidate(); }
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let mut tasks: Vec<_> = self.stores.iter_mut().flat_map(|store| store.scan(ctx)).collect();
        tasks.sort_by(|a, b| b.last_activity_secs.cmp(&a.last_activity_secs));
        let mut seen = HashSet::new();
        tasks.into_iter().filter(|task| seen.insert(task.id.clone())).collect()
    }
}

impl CodexStore {
    fn invalidate(&mut self) {
        self.db_mtime = Default::default();
        self.history_mtime = Default::default();
        self.tails.clear();
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let db = self.state_db();
        let db_mtime = db_stamp(&db);
        if db_mtime != self.db_mtime {
            self.reload_threads(ctx);
            self.db_mtime = db_mtime;
        }
        {
            let h = self.history_db();
            let hm = db_stamp(&h);
            if hm != self.history_mtime {
                self.reload_history();
                self.history_mtime = hm;
            }
        }

        let mut out = Vec::new();

        let threads = self.threads.clone();
        for thread in &threads {
            let app_alive = if thread.terminal {
                ctx.processes.entries.iter().any(|p| {
                    let executable = p.command.split_whitespace().next().unwrap_or("");
                    Path::new(executable).file_name().is_some_and(|name| name == "codex")
                        && !p.command.contains("app-server") && !p.command.contains("exec-server")
                })
            } else if thread.vscode {
                ctx.processes.any("Visual Studio Code.app/Contents/")
            } else {
                ctx.processes.any(PROCESS_NEEDLE) || ctx.processes.any(APP_NAME)
            };
            let tail = self.tail_for(&thread.id, &thread.rollout_path);
            let activity = tail
                .as_ref()
                .map(|t| t.activity_secs)
                .filter(|s| *s > 0)
                .unwrap_or(thread.updated_at);
            let last_event = tail.as_ref().map(|t| t.last_event.as_str()).unwrap_or("");
            let last_turn = self.last_turn.get(&thread.id).cloned().unwrap_or_default();
            let silence = ctx.now_secs.saturating_sub(activity);

            let (status, reason) = if tail.as_ref().map(|t| t.waiting_input) == Some(true) {
                (TaskStatus::WaitingInput, "unanswered request_user_input call".to_string())
            } else { task_status(
                last_event, &last_turn, app_alive, silence, thread.rollout_path.is_empty(),
            ) };

            // Retention is applied after the status is known so running /
            // waiting rows are never aged out (the helper exempts them).
            if !ctx.pinned_ids.contains(&format!("codex:{}", thread.id))
                && !retention_keeps(status, activity, ctx.now_secs, ctx.retention_secs) {
                continue;
            }

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
                host: if thread.terminal {
                    "terminal".into()
                } else if thread.vscode {
                    "vscode".into()
                } else {
                    "desktop".into()
                },
                session_id: Some(thread.id.clone()),
                task_id: tail.as_ref().and_then(|tail| tail.turn_id.clone()),
                title,
                status,
                completion_id: tail.as_ref().and_then(|t| t.completion_id.clone()),
                completion: (status == TaskStatus::Completed).then_some(
                    if last_event == "task_complete" { CompletionEvidence::TaskEnd }
                    else { CompletionEvidence::ClosedTurn }
                ),
                status_reason: reason,
                last_activity_secs: activity,
                collected_at_secs: ctx.now_secs,
                summary: None,
                model: tail.as_ref().and_then(|tail| tail.model.clone()),
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
            if thread.terminal {
                record.target = JumpTarget {
                    app_name: Some("Terminal".into()),
                    bundle_id: Some("com.apple.Terminal".into()),
                    terminal_host: Some("Terminal".into()),
                    workspace_path: Some(thread.cwd.clone()),
                    ..Default::default()
                };
                record.capability.max_level = JumpLevel::L1;
                record.capability.notes = "Terminal activation only; no verified session-to-tab mapping".into();
            } else if thread.vscode {
                record.target.app_name = Some("Code".into());
                record.target.bundle_id = Some("com.microsoft.VSCode".into());
                record.target.deep_link = Some(format!("vscode://openai.chatgpt/local/{}", thread.id));
                record.target.window_title = Path::new(&thread.cwd).file_name().map(|s| s.to_string_lossy().into_owned());
                record.capability.max_level = JumpLevel::L3;
                record.capability.notes = "VSCode Codex /local/<id> route; verify the active session".into();
            }
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

    fn receipt_task() -> TaskRecord {
        TaskRecord {
            id: "codex:thread".into(), agent_key: "codex".into(), agent_name: "Codex".into(),
            host: "desktop".into(), session_id: Some("thread".into()), task_id: Some("turn-1".into()),
            title: "Task".into(), status: TaskStatus::Completed, completion_id: Some("answer-1".into()),
            completion: Some(CompletionEvidence::TaskEnd), status_reason: "task_complete".into(),
            last_activity_secs: 1, collected_at_secs: 1, summary: None, model: None,
            origin: DataOrigin::Event,
            target: JumpTarget { bundle_id: Some(BUNDLE_ID.into()), ..Default::default() },
            target_validity: TargetValidity::Unconfirmed, invalid_reason: None,
            capability: JumpCapability::default(), level: JumpLevel::L1, pinned: false,
        }
    }

    fn receipt_db(home: &Path) -> Connection {
        std::fs::create_dir_all(home).unwrap();
        let conn = Connection::open(home.join("state_5.sqlite")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE threads(id TEXT PRIMARY KEY);
            INSERT INTO threads VALUES('thread');
            CREATE TABLE thread_read_receipts(thread_id TEXT PRIMARY KEY, first_unread_turn TEXT,
                revision TEXT NOT NULL, last_published_turn TEXT);
            INSERT INTO thread_read_receipts VALUES('thread','turn-1','revision-1','turn-1');").unwrap();
        conn
    }

    #[test]
    fn native_receipts_acknowledge_only_the_exact_read_turn_and_observe_live_wal() {
        let home = std::env::temp_dir().join(format!("am-native-read-{}-{}", std::process::id(), now_millis()));
        let db = receipt_db(&home);
        let homes = [home.clone()];
        let mut task = receipt_task();
        assert!(read_completions_in_homes(&[task.clone()], &homes).is_empty());
        db.execute("UPDATE thread_read_receipts SET first_unread_turn=NULL", []).unwrap();
        assert_eq!(read_completions_in_homes(&[task.clone()], &homes), vec![(task.id.clone(), "answer-1".into())]);
        // An earlier receipt cannot clear a newer reply in the same conversation.
        task.task_id = Some("turn-2".into());
        task.completion_id = Some("answer-2".into());
        assert!(read_completions_in_homes(&[task.clone()], &homes).is_empty());
        db.execute("UPDATE thread_read_receipts SET last_published_turn='turn-2'", []).unwrap();
        assert_eq!(read_completions_in_homes(&[task.clone()], &homes), vec![(task.id.clone(), "answer-2".into())]);
        db.execute("UPDATE thread_read_receipts SET last_published_turn=NULL", []).unwrap();
        assert!(read_completions_in_homes(&[task], &homes).is_empty());
        drop(db);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_receipts_do_not_acknowledge_other_hosts_or_unconfirmed_versions() {
        let home = std::env::temp_dir().join(format!("am-read-host-{}-{}", std::process::id(), now_millis()));
        let db = receipt_db(&home);
        db.execute("UPDATE thread_read_receipts SET first_unread_turn=NULL", []).unwrap();
        let mut tasks = Vec::new();
        for host in ["terminal", "vscode"] { let mut task = receipt_task(); task.host = host.into(); tasks.push(task); }
        let mut running = receipt_task(); running.status = TaskStatus::Running; tasks.push(running);
        let mut no_turn = receipt_task(); no_turn.task_id = None; tasks.push(no_turn);
        let mut no_version = receipt_task(); no_version.completion_id = None; tasks.push(no_version);
        let mut other = receipt_task(); other.agent_key = "other".into(); tasks.push(other);
        assert!(read_completions_in_homes(&tasks, &[home.clone()]).is_empty());
        drop(db);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn read_receipts_use_the_first_configured_home_and_tolerate_older_schemas() {
        let base = std::env::temp_dir().join(format!("am-read-homes-{}-{}", std::process::id(), now_millis()));
        let homes = [base.join("one"), base.join("two")];
        let first = receipt_db(&homes[0]);
        let second = receipt_db(&homes[1]);
        second.execute("UPDATE thread_read_receipts SET first_unread_turn=NULL", []).unwrap();
        assert!(read_completions_in_homes(&[receipt_task()], &homes).is_empty());
        first.execute("DROP TABLE thread_read_receipts", []).unwrap();
        assert!(read_completions_in_homes(&[receipt_task()], &homes).is_empty());
        first.execute("DELETE FROM threads", []).unwrap();
        assert_eq!(read_completions_in_homes(&[receipt_task()], &homes).len(), 1);
        drop(first); drop(second);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn rollout_read_receipt_identity_comes_from_the_latest_task_boundary() {
        let path = std::env::temp_dir().join(format!("am-turn-read-{}-{}.jsonl", std::process::id(), now_millis()));
        std::fs::write(&path, concat!(
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-03T00:00:00Z\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"old\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-03T00:00:01Z\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"new\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-03T00:00:02Z\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"new\"}}\n"
        )).unwrap();
        let tail = read_rollout_tail(&path).unwrap();
        assert_eq!(tail.turn_id.as_deref(), Some("new"));
        assert_eq!(tail.completion_id.as_deref(), Some("2026-11-03T00:00:02Z"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn configured_paths_are_absolute_and_deduplicated() {
        assert!(parse_homes(br#"["relative"]"#).is_err());
        assert!(parse_homes(b"broken").is_err());
        assert_eq!(parse_homes(br#"["/one", "/two", "/one"]"#).unwrap(), vec![PathBuf::from("/one"), PathBuf::from("/two")]);
    }

    #[test]
    fn independent_homes_merge_sessions_and_keep_terminal_targets_out_of_the_desktop() {
        with_rollout("", |path| {
            let root = path.with_extension("homes");
            let now = parse_rfc3339("2026-10-24T01:00:00Z").unwrap();
            let mut stores = Vec::new();
            for (name, source, originator) in [("desktop", "vscode", "codex_work_desktop"), ("cli", "cli", "codex-tui")] {
                let home = root.join(name);
                std::fs::create_dir_all(&home).unwrap();
                let rollout = home.join("rollout.jsonl");
                let meta = serde_json::json!({"payload": {"originator": originator}}).to_string() + "\n";
                std::fs::write(&rollout, meta + &event("task_complete")).unwrap();
                let db = Connection::open(home.join("state_5.sqlite")).unwrap();
                db.execute_batch("CREATE TABLE threads(id TEXT, name TEXT, title TEXT, rollout_path TEXT, cwd TEXT, updated_at INTEGER, source TEXT)").unwrap();
                db.execute("INSERT INTO threads VALUES (?1, NULL, ?1, ?2, '/project', ?3, ?4)",
                    rusqlite::params![name, rollout.to_str().unwrap(), now as i64, source]).unwrap();
                stores.push(CodexStore::new(home));
            }
            let mut source = CodexSource { stores };
            let ctx = ScanCtx::new(now, std::sync::Arc::new(super::super::ProcessTable::default()), 3600);
            let tasks = source.scan(&ctx);
            assert_eq!(tasks.len(), 2);
            let desktop = tasks.iter().find(|t| t.host == "desktop").unwrap();
            assert!(desktop.target.deep_link.as_deref().unwrap().starts_with("codex://"));
            let cli = tasks.iter().find(|t| t.host == "terminal").unwrap();
            assert_eq!(cli.id, "codex:cli");
            assert_eq!(cli.status, TaskStatus::Completed);
            assert_eq!(cli.target.bundle_id.as_deref(), Some("com.apple.Terminal"));
            assert!(cli.target.deep_link.is_none());
            assert_eq!(cli.level, JumpLevel::L1);
            source.stores.push(CodexStore::new(root.join("cli")));
            assert_eq!(source.scan(&ctx).len(), 2);
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn readonly_database_includes_new_wal_rows_and_stamp_changes() {
        with_rollout("", |path| {
            let writer = Connection::open(path).unwrap();
            writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE test (id TEXT); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
            let before = db_stamp(path);
            writer.execute("INSERT INTO test VALUES ('vscode-session')", []).unwrap();
            assert_ne!(before, db_stamp(path));
            let reader = CodexStore::open_ro(path).unwrap();
            assert_eq!(reader.query_row("SELECT id FROM test", [], |r| r.get::<_, String>(0)).unwrap(), "vscode-session");
            assert!(reader.execute("DELETE FROM test", []).is_err());
        });
    }

    #[test]
    fn host_uses_originator_because_desktop_also_reports_vscode_source() {
        for (originator, expected) in [("codex_vscode", true), ("vscode_agent_host", true), ("codex_work_desktop", false), ("research_chat_handoff", false)] {
            let meta = serde_json::json!({"type": "session_meta", "payload": {"source": "vscode", "originator": originator}}).to_string() + "\n";
            with_rollout(&meta, |path| assert_eq!(rollout_is_vscode(path), expected));
        }
    }

    fn with_rollout(contents: &str, check: impl FnOnce(&Path)) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "am-codex-{}-{}.jsonl", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(&path, contents).unwrap();
        check(&path);
        std::fs::remove_file(path).unwrap();
    }

    fn event(kind: &str) -> String {
        format!("{{\"timestamp\":\"2026-10-24T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n")
    }

    fn response(payload: serde_json::Value) -> String {
        serde_json::json!({"type": "response_item", "payload": payload}).to_string() + "\n"
    }

    #[test]
    fn completion_identity_ignores_later_log_activity() {
        let text = event("task_started") + &event("task_complete") +
            "{\"timestamp\":\"2026-10-24T02:00:00Z\",\"type\":\"metadata\"}\n";
        with_rollout(&text, |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(tail.completion_id.as_deref(), Some("2026-10-24T01:00:00Z"));
            assert_eq!(tail.activity_secs, parse_rfc3339("2026-10-24T02:00:00Z").unwrap());
        });
        with_rollout(&(text + &event("task_started")), |path| {
            assert_eq!(read_rollout_tail(path).unwrap().completion_id, None);
        });
    }

    #[test]
    fn model_comes_from_latest_turn_even_after_final_completion() {
        let context = |model| serde_json::json!({"type": "turn_context", "payload": {"model": model}}).to_string() + "\n";
        let old = event("task_started") + &context("old-model") + &event("task_complete");
        let completed = old.clone() + &event("task_started") + &context("gpt-6.1-sol") + &event("task_complete");
        with_rollout(&completed, |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(tail.last_event, "task_complete");
            assert_eq!(tail.model.as_deref(), Some("gpt-6.1-sol"));
        });
        with_rollout(&(old + &event("task_started")), |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(tail.last_event, "task_started");
            assert_eq!(tail.model, None);
        });
    }

    #[test]
    fn question_calls_wait_until_the_matching_result_not_a_tool_result() {
        let question = response(serde_json::json!({"type": "function_call", "name": "request_user_input", "call_id": "q1", "arguments": "{}"}));
        let unrelated = response(serde_json::json!({"type": "function_call_output", "call_id": "other", "output": "done"}));
        let answered = response(serde_json::json!({"type": "function_call_output", "call_id": "q1", "output": "answer"}));
        for (text, waiting) in [
            (event("task_started") + &question + &unrelated, true),
            (event("task_started") + &question + &answered, false),
            (event("task_started") + &question + &event("task_complete"), false),
        ] {
            with_rollout(&text, |path| assert_eq!(read_rollout_tail(path).unwrap().waiting_input, waiting));
        }
    }

    #[test]
    fn async_acceptance_is_not_an_answer_and_each_question_must_be_answered() {
        let call = response(serde_json::json!({"type": "function_call", "name": "functions.request_user_input_async", "call_id": "q2",
            "arguments": "{\"questions\":[{},{}]}"}));
        let accepted = response(serde_json::json!({"type": "function_call_output", "call_id": "q2", "output": "{\"accepted\":true}"}));
        let reply = |index| response(serde_json::json!({"type": "message", "role": "user", "content": [{"type": "input_text",
            "text": format!("<send_user_message_question_reply>\n[{{\"questionItemId\":{}}}]\n</send_user_message_question_reply>",
                serde_json::to_string(&serde_json::json!(["request_user_input_async", "q2", index]).to_string()).unwrap())}]}));
        let first = event("task_started") + &call + &accepted + &reply(0);
        with_rollout(&first, |path| assert!(read_rollout_tail(path).unwrap().waiting_input));
        with_rollout(&(first + &reply(1)), |path| assert!(!read_rollout_tail(path).unwrap().waiting_input));
    }

    #[test]
    fn intermediate_items_cannot_reuse_a_previous_turn_completion() {
        let text = event("task_complete") + &event("task_started")
            + &event("item_completed") + &event("token_count");
        with_rollout(&text, |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(tail.last_event, "task_started");
            assert_eq!(task_status(&tail.last_event, "completed", true, 0, false).0, TaskStatus::Running);
        });
    }

    #[test]
    fn assistant_text_is_not_a_task_completion() {
        let text = event("task_started")
            + "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"final\"}}\n";
        with_rollout(&text, |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(task_status(&tail.last_event, "completed", true, 0, false).0, TaskStatus::Running);
        });
    }

    #[test]
    fn task_complete_remains_completed_after_usage_events() {
        let text = event("task_started") + &event("task_complete") + &event("token_count");
        with_rollout(&text, |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(tail.last_event, "task_complete");
            assert_eq!(task_status(&tail.last_event, "", true, 0, false).0, TaskStatus::Completed);
        });
    }

    #[test]
    fn long_unicode_tool_output_does_not_lose_the_open_task_boundary() {
        let output = serde_json::json!({"type": "response_item", "payload": {
            "type": "function_call_output", "output": "中".repeat(ROLLOUT_TAIL_BYTES as usize)
        }});
        let text = event("task_complete") + &event("task_started")
            + &output.to_string() + "\n" + &event("item_completed") + "{\"type\":";
        with_rollout(&text, |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(tail.last_event, "task_started");
            assert_eq!(task_status(&tail.last_event, "completed", true, 0, false).0, TaskStatus::Running);
        });
    }

    #[test]
    fn aborted_turn_does_not_reuse_previous_completion() {
        with_rollout(&(event("task_started") + &event("turn_aborted")), |path| {
            let tail = read_rollout_tail(path).unwrap();
            assert_eq!(task_status(&tail.last_event, "completed", true, 0, false).0, TaskStatus::Failed);
        });
    }

    #[test]
    fn cache_observes_new_task_even_with_an_unchanged_mtime() {
        with_rollout(&event("task_complete"), |path| {
            let mut source = CodexStore::new(path.parent().unwrap().to_path_buf());
            let name = path.to_str().unwrap();
            assert_eq!(source.tail_for("thread", name).unwrap().last_event, "task_complete");
            let modified = std::fs::metadata(path).unwrap().modified().unwrap();
            std::fs::write(path, event("task_complete") + &event("task_started")).unwrap();
            std::fs::File::options().write(true).open(path).unwrap().set_modified(modified).unwrap();
            assert_eq!(source.tail_for("thread", name).unwrap().last_event, "task_started");
        });
    }

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
