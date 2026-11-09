//! ZCode adapter.
//!
//! Two sources, each with a distinct role:
//!
//! 1. `~/.zcode/v2/tasks-index.sqlite`, table `tasks` — the task list itself
//!    (task_id, title, workspace, model, created_at/updated_at in ms).
//!    Read-only, WAL-aware, fingerprinted (see below).
//! 2. `~/.zcode/cli/db/db.sqlite`, tables `message` + `part` — the transcript.
//!    This is where **result evidence** lives: the chronologically last
//!    non-synthetic part of a session tells how the agent's work actually
//!    ended. `step-finish` with `reason="stop"` is a delivered final answer
//!    (→ Completed, `AssistantEndTurn`); an abnormal finish reason
//!    (`content-filter`, `other`) is a failed delivery (→ Failed); anything
//!    else means the session is mid-turn or waiting (→ freshness decides).
//!
//! Why the transcript decides and not the index: `tasks.task_status` is
//! ZCode's **process-level** stamp — it records how the last turn happened,
//! not how the session ended. Observed on this host: a session whose turn
//! errored at 12:22 and 12:56 kept working and delivered at 13:22, yet the
//! index still reads `error`; a live session reads `completed` between
//! turns. Trusting that literal misreports recovered sessions as failed and
//! in-flight ones as done. The index literal is therefore only a fallback
//! for sessions the transcript does not cover, never the primary evidence.
//!
//! The database is WAL-mode: between checkpoints, committed rows live in the
//! `-wal` sidecar. `immutable=1` silently ignores that sidecar and serves a
//! stale snapshot, so we open WAL-aware (`mode=ro`) and fingerprint the
//! `-wal`/`-shm` mtimes too. When no writer is around to serve a read-only
//! WAL open (crashed app, leftover `-wal`), we fall back to a private
//! db+wal copy.
//!
//! Status vocabulary observed on this machine: a live session keeps
//! `task_status` NULL/empty; `'completed'`/`'error'` are process stamps (see
//! above). An empty status with fresh `updated_at` therefore means Running
//! (ZCode touches the row on every turn); once activity goes quiet the
//! transcript tail decides.
//!
//! Model ids arrive provider-scoped (`account:zai-start-plan/GLM-5.3-Flash`);
//! only the model name after the last `/` is displayed.

use super::{retention_keeps, truncate_chars, ScanCtx, Source};
use crate::model::*;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const APP_NAME: &str = "ZCode";
const BUNDLE_ID: &str = "dev.zcode.app";
const PROCESS_NEEDLE: &str = "ZCode.app/Contents/MacOS/ZCode";
const MAX_JUMP_LEVEL: JumpLevel = if cfg!(target_os = "macos") { JumpLevel::L3 } else { JumpLevel::L2 };

pub(crate) struct SessionJumpTarget {
    pub title: String,
    pub indexed_titles: Vec<String>,
}

pub(crate) fn normalized_title(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Resolve the requested ID against the live index, including titles outside
/// the monitor's retention window. Never substitute a cached/custom UI title.
pub(crate) fn session_jump_target(task_id: &str) -> Result<SessionJumpTarget, String> {
    let path = ZCodeSource::db_path().ok_or("task_gone")?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| "zcode_index_unavailable")?;
    session_jump_target_in(&conn, task_id).map_err(str::to_string)
}

fn session_jump_target_in(conn: &Connection, task_id: &str) -> Result<SessionJumpTarget, &'static str> {
    let mut statement = conn.prepare("select task_id, coalesce(title,''), coalesce(archived,0) from tasks where coalesce(deleted,0)=0")
        .map_err(|_| "zcode_index_unavailable")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)))
        .map_err(|_| "zcode_index_unavailable")?
        .collect::<Result<Vec<_>, _>>().map_err(|_| "zcode_index_unavailable")?;
    let targets: Vec<_> = rows.iter().filter(|(id, _, archived)| id == task_id && *archived == 0).collect();
    if targets.is_empty() { return Err("task_gone"); }
    if targets.len() != 1 { return Err("zcode_session_ambiguous"); }
    let title = normalized_title(&targets[0].1);
    let indexed_titles: Vec<_> = rows.iter().map(|(_, title, _)| normalized_title(title)).filter(|title| !title.is_empty()).collect();
    if title.is_empty() || indexed_titles.iter().filter(|candidate| **candidate == title).count() != 1 {
        return Err("zcode_session_ambiguous");
    }
    Ok(SessionJumpTarget { title, indexed_titles })
}

/// A live ZCode session touches its index row on every turn, so an empty
/// `task_status` counts as Running only while `updated_at` is this fresh.
/// Long enough to ride out a slow tool call, short enough that a finished
/// turn does not linger as "running" for ages.
const ACTIVE_FRESHNESS_SECS: u64 = 300;

/// `step-finish` reasons that mean the turn could not deliver its result.
/// `"stop"` is the normal end-of-turn; `"tool-calls"` is mid-process.
const ABNORMAL_FINISH_REASONS: [&str; 2] = ["content-filter", "other"];

struct TaskRow {
    workspace_path: String,
    task_id: String,
    title: String,
    task_status: String,
    model: String,
    created_at: u64,
    updated_at: u64,
}

/// Result evidence from the transcript tail of one session. `None`/empty
/// fields mean "not present"; the decision function treats missing evidence
/// honestly rather than guessing.
#[derive(Clone, Debug, Default, PartialEq)]
struct SessionTail {
    /// Chronologically last non-synthetic message role ("user"/"assistant").
    last_role: String,
    /// Type of the last part (`step-finish`, `tool`, `text`, …).
    last_part: String,
    /// `step-finish` reason when the last part is a step-finish.
    finish_reason: String,
    /// Reason of the **last `step-finish` part anywhere in the tail** — the
    /// true end-of-turn marker. The last part of a session is not always the
    /// turn's ending: async artifacts (a `tool` part flushed after the turn
    /// closed) and queued user messages can follow a delivered answer, so
    /// the decision must look at the turn's finish, not the last row.
    last_finish_reason: String,
    /// Message id of the last part; same as `last_finish_msg_id` when
    /// nothing followed the turn's close (trailing same-message artifacts
    /// are part of the delivered turn, not new work).
    last_msg_id: String,
    last_finish_msg_id: String,
    /// Last assistant text part — the agent's most recent visible words.
    last_assistant_text: Option<String>,
    /// Time of the last message (ms epoch), the liveness signal.
    last_time_ms: u64,
}

pub struct ZCodeSource {
    db_fingerprint: u64,
    transcript_fingerprint: u64,
    rows: Vec<TaskRow>,
    tails: HashMap<String, SessionTail>,
}

impl ZCodeSource {
    pub fn new() -> Self {
        Self {
            db_fingerprint: 0,
            transcript_fingerprint: 0,
            rows: Vec::new(),
            tails: HashMap::new(),
        }
    }

    fn db_path() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".zcode/v2/tasks-index.sqlite"))
    }

    fn transcript_path() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".zcode/cli/db/db.sqlite"))
    }

    /// Fingerprint that changes whenever the index content can have changed.
    /// WAL mode writes land in `tasks-index.sqlite-wal`; the main database
    /// file's mtime only moves on checkpoint, so watching it alone hides
    /// live updates between checkpoints.
    fn db_fingerprint() -> Option<u64> {
        Self::wal_fingerprint(&Self::db_path()?)
    }

    fn transcript_fingerprint() -> Option<u64> {
        Self::wal_fingerprint(&Self::transcript_path()?)
    }

    fn wal_fingerprint(db: &Path) -> Option<u64> {
        let base = db.to_string_lossy().into_owned();
        let mut newest = super::file_mtime_secs(db);
        for suffix in ["-wal", "-shm"] {
            newest = newest.max(super::file_mtime_secs(&PathBuf::from(format!(
                "{base}{suffix}"
            ))));
        }
        Some(newest)
    }

    /// WAL-aware read-only open: `mode=ro` sees committed WAL content without
    /// blocking the writer. (`immutable=1` was wrong here — it ignores the
    /// WAL and serves a stale snapshot.) A read-only connection cannot
    /// recover a WAL when no writer is around (crashed app, leftover
    /// `-wal`), and that failure only surfaces at first query — hence the
    /// probe below. Fall back to a private db+wal copy for that case.
    fn open_ro(path: &Path) -> Option<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let uri = format!("file:{}?mode=ro", uri_path(path));
        if let Ok(conn) = Connection::open_with_flags(uri, flags) {
            if conn
                .query_row("select count(*) from sqlite_master", [], |_| Ok(()))
                .is_ok()
            {
                return Some(conn);
            }
        }
        Self::open_private_copy(path)
    }

    /// Copy db + wal to a scratch directory and open the copy read-write so
    /// SQLite performs WAL recovery there; the original stays untouched.
    fn open_private_copy(path: &Path) -> Option<Connection> {
        let dir = std::env::temp_dir().join(format!("agent-monitor-zcode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let base = path.to_string_lossy().into_owned();
        let mut copied = false;
        for suffix in ["", "-wal"] {
            let src = PathBuf::from(format!("{base}{suffix}"));
            if src.exists() {
                let dst = dir.join(format!("tasks-index.sqlite{suffix}"));
                if std::fs::copy(&src, &dst).is_err() {
                    return None;
                }
                copied = true;
            }
        }
        if !copied {
            return None;
        }
        Connection::open(dir.join("tasks-index.sqlite")).ok()
    }

    fn reload(&mut self) {
        let Some(path) = Self::db_path() else {
            return;
        };
        let Some(conn) = Self::open_ro(&path) else {
            return;
        };
        let mut rows = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "select workspace_path, task_id, title, coalesce(task_status,''), coalesce(model,''),
                    created_at, updated_at
             from tasks
             where coalesce(deleted,0) = 0 and coalesce(archived,0) = 0
             order by updated_at desc limit 80",
        ) {
            if let Ok(mapped) = stmt.query_map([], |row| {
                Ok(TaskRow {
                    workspace_path: row.get(0)?,
                    task_id: row.get(1)?,
                    title: row.get(2)?,
                    task_status: row.get(3)?,
                    model: row.get(4)?,
                    created_at: row.get::<_, i64>(5)? as u64,
                    updated_at: row.get::<_, i64>(6)? as u64,
                })
            }) {
                rows.extend(mapped.flatten());
            }
        }
        self.rows = rows;
    }

    /// Load the transcript tail for every indexed session, in one grouped
    /// query. The last non-synthetic part carries the result evidence; the
    /// last assistant text part doubles as the row summary. `rowid` breaks
    /// time ties because `sequence` is nullable on older rows.
    fn load_tails(&mut self, session_ids: &[String]) {
        self.tails.clear();
        if session_ids.is_empty() {
            return;
        }
        let Some(path) = Self::transcript_path() else {
            return;
        };
        let Some(conn) = Self::open_ro(&path) else {
            eprintln!("zcode: transcript open failed: {}", path.display());
            return;
        };
        let placeholders = session_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "with tail_part as (
                 select p.session_id, p.data, p.time_created, p.message_id,
                        row_number() over (partition by p.session_id
                                           order by p.time_created desc, p.rowid desc) rn
                 from part p
                 where p.session_id in ({placeholders})
                   and coalesce(json_extract(p.data,'$.synthetic'),0) = 0
             ),
             tail_msg as (
                 select m.session_id, m.data, m.time_created,
                        row_number() over (partition by m.session_id
                                           order by m.time_created desc, m.rowid desc) rn
                 from message m
                 where m.session_id in ({placeholders})
             ),
             last_finish as (
                 select p.session_id, p.data, p.message_id,
                        row_number() over (partition by p.session_id
                                           order by p.time_created desc, p.rowid desc) rn
                 from part p
                 where p.session_id in ({placeholders})
                   and json_extract(p.data,'$.type') = 'step-finish'
                   and coalesce(json_extract(p.data,'$.synthetic'),0) = 0
             ),
             last_text as (
                 select p.session_id, p.data,
                        row_number() over (partition by p.session_id
                                           order by p.time_created desc, p.rowid desc) rn
                 from part p join message m on m.id = p.message_id
                 where p.session_id in ({placeholders})
                   and json_extract(m.data,'$.role') = 'assistant'
                   and json_extract(p.data,'$.type') = 'text'
                   and coalesce(json_extract(p.data,'$.synthetic'),0) = 0
             )
             select tp.session_id, tp.data, tp.time_created, tp.message_id, tm.data, lf.data, lf.message_id, lt.data
             from tail_part tp
             join tail_msg tm on tm.session_id = tp.session_id and tm.rn = 1
             left join last_finish lf on lf.session_id = tp.session_id and lf.rn = 1
             left join last_text lt on lt.session_id = tp.session_id and lt.rn = 1
             where tp.rn = 1",
        );
        let Ok(mut stmt) = conn.prepare(&sql) else {
            eprintln!("zcode: transcript tail prepare failed");
            return;
        };
        // The same id list is bound four times: once per CTE.
        let n = session_ids.len();
        let mut query = |stmt: &mut rusqlite::Statement| -> rusqlite::Result<()> {
            for group in 0..4 {
                for (j, id) in session_ids.iter().enumerate() {
                    stmt.raw_bind_parameter(group * n + j + 1, id)?;
                }
            }
            let mut rows = stmt.raw_query();
            while let Some(row) = rows.next()? {
                let session_id: String = row.get(0)?;
                let part_data: String = row.get(1)?;
                let last_time_ms: u64 = row.get::<_, i64>(2)?.max(0) as u64;
                let last_msg_id: String = row.get(3)?;
                let msg_data: String = row.get(4)?;
                let finish_data: Option<String> = row.get(5)?;
                let finish_msg_id: Option<String> = row.get(6)?;
                let text_data: Option<String> = row.get(7)?;
                let tail = SessionTail {
                    last_role: json_str(&msg_data, "role"),
                    last_part: json_str(&part_data, "type"),
                    finish_reason: json_str(&part_data, "reason"),
                    last_finish_reason: finish_data
                        .map(|d| json_str(&d, "reason"))
                        .unwrap_or_default(),
                    last_msg_id,
                    last_finish_msg_id: finish_msg_id.unwrap_or_default(),
                    last_assistant_text: text_data.and_then(|t| {
                        let text = json_str(&t, "text");
                        (!text.trim().is_empty()).then(|| truncate_chars(text.trim(), 120))
                    }),
                    last_time_ms,
                };
                self.tails.insert(session_id, tail);
            }
            Ok(())
        };
        if let Err(err) = query(&mut stmt) {
            eprintln!("zcode: transcript tail query failed: {err}");
        }
    }
}

/// Read a top-level transcript value with normal JSON escaping rules.
fn json_str(json: &str, field: &str) -> String {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(json) else {
        return String::new();
    };
    match data.get(field) {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F")
}

/// Map ZCode's index status to the unified status. Never infers a waiting
/// state: only explicit waiting_* literals (unobserved here) map to them.
fn map_status(s: &str) -> (TaskStatus, String) {
    let key = s.trim().to_ascii_lowercase();
    match key.as_str() {
        "completed" => {
            (TaskStatus::Completed, "task_status = completed".into())
        }
        "failed" | "error" => (TaskStatus::Failed, "task_status = failed".into()),
        "cancelled" | "canceled" => (TaskStatus::Failed, "task_status = cancelled".into()),
        "running" | "in_progress" | "working" | "executing" => {
            (TaskStatus::Running, format!("task_status = {key}"))
        }
        "waiting_approval" | "awaiting_approval" => (
            TaskStatus::WaitingApproval,
            format!("task_status = {key}"),
        ),
        "waiting_input" | "awaiting_input" | "needs_input" => (
            TaskStatus::WaitingInput,
            format!("task_status = {key}"),
        ),
        "idle" => (TaskStatus::Idle, "task_status = idle".into()),
        other => (TaskStatus::Unknown, format!("unmapped task_status = {other}")),
    }
}

/// Status decision for one indexed row, driven by **result evidence**.
///
/// Priority order:
/// 1. The index row is being touched right now (`task_status` empty, fresh
///    `updated_at`) → the session is live → Running. ZCode touches the row
///    on every turn, so this is the cheapest liveness signal.
/// 2. The last `step-finish` in the transcript tail is the turn's real
///    ending — the last *part* is not: async artifacts (a `tool` part
///    flushed after the turn closed) and queued user messages can follow a
///    delivered answer.
///    - `stop` with nothing but same-message artifacts after it, or with
///      only stale follow-up messages → Completed (`AssistantEndTurn`);
///    - an abnormal reason (`content-filter`, `other`) as the latest agent
///      word → Failed;
///    - anything else → freshness decides: fresh → Running, stale → Unknown.
/// 3. No transcript evidence at all → fall back to the index literal, which
///    is ZCode's process-level stamp and known to lag reality. Evidence for
///    such completions is `TaskIndex` — the weakest kind, fallback only.
///
/// Never infers a waiting state: ZCode exposes none in either source.
fn decide_row_status(
    task_status: &str,
    updated_secs: u64,
    now_secs: u64,
    tail: Option<&SessionTail>,
) -> (TaskStatus, String, Option<CompletionEvidence>) {
    if task_status.trim().is_empty()
        && now_secs.saturating_sub(updated_secs) <= ACTIVE_FRESHNESS_SECS
    {
        let age = now_secs.saturating_sub(updated_secs);
        return (
            TaskStatus::Running,
            format!("task_status empty, updated {age}s ago (live session)"),
            None,
        );
    }
    let Some(tail) = tail else {
        let (status, reason) = map_row_status(task_status, updated_secs, now_secs);
        let evidence = (status == TaskStatus::Completed).then_some(CompletionEvidence::TaskIndex);
        return (status, reason, evidence);
    };
    let tail_age = now_secs.saturating_sub(tail.last_time_ms / 1000);
    let finish_is_last_row = tail.last_finish_msg_id == tail.last_msg_id;

    // The turn closed with a delivered answer and nothing agent-side has
    // happened since (trailing same-message artifacts included).
    if tail.last_finish_reason == "stop" && finish_is_last_row {
        return (
            TaskStatus::Completed,
            format!("assistant delivered final answer (step-finish stop, {tail_age}s ago)"),
            Some(CompletionEvidence::AssistantEndTurn),
        );
    }
    // The last agent word is an abnormal turn end: a queued user message
    // after it does not revive the session — the result was not delivered.
    if ABNORMAL_FINISH_REASONS.contains(&tail.last_finish_reason.as_str()) && finish_is_last_row {
        return (
            TaskStatus::Failed,
            format!(
                "turn ended abnormally ({}, {tail_age}s ago); no activity since",
                tail.last_finish_reason
            ),
            None,
        );
    }
    let live = tail_age <= ACTIVE_FRESHNESS_SECS;
    if live {
        return (
            TaskStatus::Running,
            format!(
                "transcript active {tail_age}s ago ({} {})",
                tail.last_role, tail.last_part
            ),
            None,
        );
    }
    if tail.last_finish_reason == "stop" {
        // A delivered answer stands even when follow-up messages went
        // nowhere (queued user message, stalled new turn).
        return (
            TaskStatus::Completed,
            format!("assistant delivered final answer (step-finish stop, {tail_age}s ago)"),
            Some(CompletionEvidence::AssistantEndTurn),
        );
    }
    (
        TaskStatus::Unknown,
        format!(
            "no activity for {tail_age}s ({} {})",
            tail.last_role, tail.last_part
        ),
        None,
    )
}

/// Status decision for one indexed row. ZCode keeps `task_status` NULL/empty
/// while a session is live and stamps `'completed'` only when it ends, so an
/// empty status with fresh `updated_at` means Running — anything else would
/// misreport every in-flight session as Unknown and let retention age it out.
/// Once activity goes quiet we honestly report Unknown: a crashed session and
/// an idle one look identical in the index.
fn map_row_status(task_status: &str, updated_secs: u64, now_secs: u64) -> (TaskStatus, String) {
    if task_status.trim().is_empty() {
        let age = now_secs.saturating_sub(updated_secs);
        if age <= ACTIVE_FRESHNESS_SECS {
            (
                TaskStatus::Running,
                format!("task_status empty, updated {age}s ago (live session)"),
            )
        } else {
            (
                TaskStatus::Unknown,
                format!("task_status empty, no activity for {age}s"),
            )
        }
    } else {
        map_status(task_status)
    }
}

/// Short display model name. ZCode records provider-scoped ids like
/// `account:zai-start-plan/GLM-5.3-Flash`; the account/plan qualifier is
/// noise in a pet row, so only the model name after the last `/` is shown.
/// Plain ids pass through unchanged.
fn short_model(model: &str) -> String {
    let trimmed = model.trim();
    let without_account = trimmed.strip_prefix("account:").unwrap_or(trimmed);
    match without_account.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ => without_account.to_string(),
    }
}

/// Retention decision for one indexed task: active rows (running/waiting) are
/// exempt through the shared helper, everything ended follows the user-configured
/// window. There is deliberately no "keep the newest N rows" fallback: that
/// showed sessions weeks older than the configured retention forever.
fn keeps_row(
    status: TaskStatus,
    updated_secs: u64,
    now_secs: u64,
    retention_secs: u64,
) -> bool {
    retention_keeps(status, updated_secs, now_secs, retention_secs)
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

impl Source for ZCodeSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "zcode".into(),
            name: "ZCode".into(),
            source: "~/.zcode/v2/tasks-index.sqlite (tasks table)".into(),
            origin: DataOrigin::Index,
            max_level: MAX_JUMP_LEVEL,
            notes: "macOS: select an exact, uniquely named indexed session through ZCode's task search; verify the active conversation heading. Unnamed or ambiguous sessions are refused.".into(),
        }
    }

    fn invalidate(&mut self) {
        self.db_fingerprint = 0;
        self.transcript_fingerprint = 0;
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let fingerprint = Self::db_fingerprint().unwrap_or(0);
        if fingerprint != self.db_fingerprint {
            self.reload();
            self.db_fingerprint = fingerprint;
            // Rows may be brand new; their tails must be fetched too.
            self.transcript_fingerprint = 0;
        }
        if !self.rows.is_empty() {
            let transcript_fingerprint = Self::transcript_fingerprint().unwrap_or(0);
            if transcript_fingerprint != self.transcript_fingerprint {
                let ids: Vec<String> = self.rows.iter().map(|r| r.task_id.clone()).collect();
                self.load_tails(&ids);
                self.transcript_fingerprint = transcript_fingerprint;
            }
        }
        let app_alive = ctx.processes.any(PROCESS_NEEDLE) || ctx.processes.any(APP_NAME)
            || ctx.processes.has_executable("ZCode");
        let mut out = Vec::new();
        for row in self.rows.iter() {
            let updated = row.updated_at / 1000;
            let tail = self.tails.get(&row.task_id);
            let (status, reason, evidence) =
                decide_row_status(&row.task_status, updated, ctx.now_secs, tail);
            // Active rows are exempt inside `keeps_row`; ended ones must be
            // inside the configured retention window.
            if !ctx.pinned_ids.contains(&format!("zcode:{}", row.task_id))
                && !keeps_row(status, updated, ctx.now_secs, ctx.retention_secs) {
                continue;
            }
            let workspace = Path::new(&row.workspace_path);
            let workspace_exists = workspace.is_dir();
            let title = if row.title.trim().is_empty() {
                workspace
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "ZCode task".into())
            } else {
                truncate_chars(row.title.trim(), 120)
            };
            let mut record = TaskRecord {
                id: format!("zcode:{}", row.task_id),
                agent_key: "zcode".into(),
                agent_name: "ZCode".into(),
                host: "desktop".into(),
                session_id: Some(row.task_id.clone()),
                task_id: Some(row.task_id.clone()),
                title,
                status,
                completion_id: Some(row.task_id.clone()),
                completion: evidence,
                status_reason: reason,
                last_activity_secs: updated,
                collected_at_secs: ctx.now_secs,
                summary: tail.and_then(|t| t.last_assistant_text.clone()),
                model: (!row.model.trim().is_empty()).then(|| short_model(&row.model)),
                origin: DataOrigin::Index,
                target: JumpTarget {
                    app_name: Some(APP_NAME.into()),
                    bundle_id: Some(BUNDLE_ID.into()),
                    deep_link: (!cfg!(target_os = "macos")).then(|| format!(
                        "zcode://workspace/open?path={}",
                        percent_encode(&row.workspace_path)
                    )),
                    window_title: Some(row.title.clone()),
                    workspace_path: Some(row.workspace_path.clone()),
                    ..Default::default()
                },
                target_validity: TargetValidity::Unconfirmed,
                invalid_reason: None,
                capability: JumpCapability {
                    max_level: MAX_JUMP_LEVEL,
                    notes: "macOS L3: indexed session ID, unique current title, native task-search result and active-heading verification."
                        .into(),
                    requires: if cfg!(target_os = "macos") { vec![PermissionKind::Accessibility] } else { vec![PermissionKind::Automation] },
                },
                level: MAX_JUMP_LEVEL,
                pinned: false,
            };
            if !workspace_exists {
                record.target_validity = TargetValidity::Invalid;
                record.invalid_reason = Some("workspace_missing".into());
            } else if app_alive {
                record.target_validity = TargetValidity::Valid;
            }
            record.refresh_level();
            out.push(record);
            let _ = row.created_at;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_strings_decode_json_escapes_and_top_level_fields() {
        let json = r#"{"nested":{"role":"user"},"role" : "assistant","text":"\u4f60\u597d\n\"A\"\\B\uD83D\uDE00"}"#;
        assert_eq!(json_str(json, "role"), "assistant");
        assert_eq!(json_str(json, "text"), "你好\n\"A\"\\B😀");
        assert_eq!(json_str(json, "missing"), "");
        assert_eq!(json_str(r#"{"role":null}"#, "role"), "null");
        assert_eq!(json_str(r#"{"role":"assistant""#, "role"), "");
    }

    fn jump_index() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("create table tasks(task_id text, title text, archived integer default 0, deleted integer default 0);
            insert into tasks(task_id,title) values('a','Plan A'),('b','Plan B');").unwrap();
        conn
    }

    #[test]
    fn jump_identity_uses_id_and_current_title() {
        let conn = jump_index();
        assert_eq!(session_jump_target_in(&conn, "b").unwrap().title, "Plan B");
        conn.execute("update tasks set title=? where task_id='b'", ["Renamed 'B'"]).unwrap();
        assert_eq!(session_jump_target_in(&conn, "b").unwrap().title, "Renamed 'B'");
    }

    #[test]
    fn jump_refuses_duplicate_and_unnamed_sessions() {
        let conn = jump_index();
        conn.execute("insert into tasks(task_id,title) values('older','  Plan   A  ')", []).unwrap();
        assert_eq!(session_jump_target_in(&conn, "a").err(), Some("zcode_session_ambiguous"));
        conn.execute("update tasks set title='' where task_id='b'", []).unwrap();
        assert_eq!(session_jump_target_in(&conn, "b").err(), Some("zcode_session_ambiguous"));
    }

    #[test]
    fn jump_refuses_missing_archived_and_deleted_sessions() {
        let conn = jump_index();
        assert_eq!(session_jump_target_in(&conn, "missing").err(), Some("task_gone"));
        conn.execute("update tasks set archived=1 where task_id='a'", []).unwrap();
        conn.execute("update tasks set deleted=1 where task_id='b'", []).unwrap();
        assert_eq!(session_jump_target_in(&conn, "a").err(), Some("task_gone"));
        assert_eq!(session_jump_target_in(&conn, "b").err(), Some("task_gone"));
    }

    /// Tail fixture: times are ms epochs relative to `now_secs`. By default
    /// the finish belongs to the same message as the last part.
    fn tail(role: &str, part: &str, reason: &str, age_secs: u64, text: Option<&str>) -> SessionTail {
        SessionTail {
            last_role: role.into(),
            last_part: part.into(),
            finish_reason: reason.into(),
            last_finish_reason: reason.into(),
            last_msg_id: "m1".into(),
            last_finish_msg_id: "m1".into(),
            last_assistant_text: text.map(|t| t.into()),
            last_time_ms: (now_secs() - age_secs) * 1000,
        }
    }

    fn now_secs() -> u64 {
        2_000_000
    }

    #[test]
    fn maps_observed_and_referenced_statuses() {
        assert_eq!(map_status("completed").0, TaskStatus::Completed);
        assert_eq!(map_status("running").0, TaskStatus::Running);
        assert_eq!(map_status("failed").0, TaskStatus::Failed);
        assert_eq!(map_status("cancelled").0, TaskStatus::Failed);
        assert_eq!(map_status("pending").0, TaskStatus::Unknown);
        assert_eq!(map_status("").0, TaskStatus::Unknown);
    }

    #[test]
    fn recovered_error_session_is_completed_not_failed() {
        // The observed bug: a session whose turn errored at 12:22/12:56 kept
        // working and delivered at 13:22, yet the index still read "error".
        // The transcript tail (stop) is the result evidence and must win.
        let (status, reason, evidence) = decide_row_status(
            "error",
            now_secs() - 6 * 60,
            now_secs(),
            Some(&tail("assistant", "step-finish", "stop", 30, Some("已完成调研"))),
        );
        assert_eq!(status, TaskStatus::Completed, "{reason}");
        assert_eq!(evidence, Some(CompletionEvidence::AssistantEndTurn));
        assert!(reason.contains("stop"));
    }

    #[test]
    fn trailing_artifact_after_stop_is_still_completed() {
        // The reported bug: the last part of the delivered turn is a `tool`
        // part flushed *after* the `step-finish:stop` (async artifact), and a
        // queued user message followed. Only looking at the last part turned
        // a delivered session into Unknown once it went stale.
        let mut t = tail("assistant", "tool", "", 600, Some("最终答复摘要"));
        // Same message: the artifact belongs to the delivered turn, whose
        // step-finish said "stop".
        t.last_finish_reason = "stop".into();
        let (status, reason, evidence) =
            decide_row_status("completed", now_secs() - 600, now_secs(), Some(&t));
        assert_eq!(status, TaskStatus::Completed, "{reason}");
        assert_eq!(evidence, Some(CompletionEvidence::AssistantEndTurn));

        // Variant: a queued user message after the delivered turn, never
        // answered. The delivered result still stands when stale.
        let mut t = tail("user", "text", "", 900, None);
        t.last_finish_reason = "stop".into();
        t.last_finish_msg_id = "m0".into();
        let (status, reason, _) =
            decide_row_status("completed", now_secs() - 900, now_secs(), Some(&t));
        assert_eq!(status, TaskStatus::Completed, "{reason}");
    }

    #[test]
    fn fresh_agent_work_after_delivery_is_running() {
        // A new turn started after the delivered answer and is producing
        // parts right now: that is Running, not Completed.
        let mut t = tail("assistant", "tool", "", 30, None);
        t.last_finish_reason = "stop".into();
        t.last_finish_msg_id = "m0".into();
        let (status, reason, _) =
            decide_row_status("completed", now_secs() - 60, now_secs(), Some(&t));
        assert_eq!(status, TaskStatus::Running, "{reason}");
    }

    #[test]
    fn live_session_between_turns_is_running_not_completed() {
        // The index stamps "completed" on turn end while the session stays
        // open; the transcript (mid-turn or user-last) must keep it honest.
        let (status, reason, _) = decide_row_status(
            "completed",
            now_secs() - 2 * 60,
            now_secs(),
            Some(&tail("assistant", "step-finish", "tool-calls", 20, None)),
        );
        assert_eq!(status, TaskStatus::Running, "{reason}");
    }

    #[test]
    fn delivered_stop_is_completed_even_when_stale() {
        // Delivered, then the user read the answer for an hour. Still
        // Completed — delivery does not expire back into Unknown.
        let (status, _, evidence) = decide_row_status(
            "",
            now_secs() - 3_600,
            now_secs(),
            Some(&tail("assistant", "step-finish", "stop", 3_600, None)),
        );
        assert_eq!(status, TaskStatus::Completed);
        assert_eq!(evidence, Some(CompletionEvidence::AssistantEndTurn));
    }

    #[test]
    fn abnormal_finish_reason_is_failed() {
        let (status, reason, evidence) = decide_row_status(
            "",
            now_secs() - 600,
            now_secs(),
            Some(&tail("assistant", "step-finish", "content-filter", 600, None)),
        );
        assert_eq!(status, TaskStatus::Failed, "{reason}");
        assert!(evidence.is_none());
    }

    #[test]
    fn stale_mid_turn_tail_is_unknown_not_running() {
        let (status, reason, _) = decide_row_status(
            "",
            now_secs() - 9 * 60,
            now_secs(),
            Some(&tail("assistant", "tool", "", 9 * 60, None)),
        );
        assert_eq!(status, TaskStatus::Unknown, "{reason}");
        assert!(reason.contains("no activity"));
    }

    #[test]
    fn no_transcript_falls_back_to_index_literal_with_weak_evidence() {
        let (status, reason, evidence) =
            decide_row_status("completed", now_secs() - 60, now_secs(), None);
        assert_eq!(status, TaskStatus::Completed, "{reason}");
        assert_eq!(evidence, Some(CompletionEvidence::TaskIndex));
    }

    #[test]
    fn index_liveness_still_wins_for_running_sessions() {
        // Empty status + fresh index touch: the cheapest liveness signal.
        let (status, reason, _) = decide_row_status(
            "",
            now_secs() - 30,
            now_secs(),
            Some(&tail("assistant", "tool", "", 30, None)),
        );
        assert_eq!(status, TaskStatus::Running, "{reason}");
        assert!(reason.contains("live session"));
    }

    #[test]
    fn json_str_handles_strings_escapes_and_scalars() {
        assert_eq!(
            json_str(r#"{"type":"step-finish","reason":"stop","tokens":{"total":1}}"#, "reason"),
            "stop"
        );
        assert_eq!(json_str(r#"{"text":"line1\nline2"}"#, "text"), "line1\nline2");
        assert_eq!(json_str(r#"{"synthetic":true,"type":"text"}"#, "synthetic"), "true");
        assert_eq!(json_str(r#"{"absent":1}"#, "missing"), "");
    }

    /// Live-data check against this machine's real ZCode stores. Ignored in
    /// CI (`cargo test -- --ignored` to run): it exists to eyeball the
    /// status decisions on real transcripts, especially recovered-error
    /// sessions and in-flight ones.
    #[test]
    #[ignore]
    fn live_scan_decides_from_real_local_data() {
        use super::super::{sort_tasks, ProcessTable};
        use std::sync::Arc;
        let mut source = ZCodeSource::new();
        let ctx = ScanCtx::new(
            crate::model::now_secs(),
            Arc::new(ProcessTable::sample()),
            72 * 3_600,
        );
        let mut records = source.scan(&ctx);
        sort_tasks(&mut records);
        println!("--- live ZCode scan: {} rows ---", records.len());
        for r in records.iter().take(12) {
            println!(
                "{:<40} {:>10}  {}",
                truncate_chars(&r.title, 38),
                format!("{:?}", r.status),
                r.status_reason
            );
            if let Some(summary) = &r.summary {
                println!("    └ summary: {}", truncate_chars(summary, 60));
            }
        }
    }

    #[test]
    fn empty_status_with_fresh_activity_is_running() {
        // Live sessions keep task_status NULL; only freshness tells them
        // apart from abandoned ones.
        let now = 2_000_000;
        let (status, reason) = map_row_status("", now - 10, now);
        assert_eq!(status, TaskStatus::Running);
        assert!(reason.contains("live session"));
        // Boundary: exactly ACTIVE_FRESHNESS_SECS old still counts as live.
        assert_eq!(
            map_row_status("", now - ACTIVE_FRESHNESS_SECS, now).0,
            TaskStatus::Running
        );
    }

    #[test]
    fn empty_status_without_activity_stays_unknown() {
        let now = 2_000_000;
        let (status, reason) = map_row_status("", now - ACTIVE_FRESHNESS_SECS - 1, now);
        assert_eq!(status, TaskStatus::Unknown);
        assert!(reason.contains("no activity"));
        // A stale row is subject to retention like other ended tasks —
        // regression guard: it must not be reported as Running to dodge it.
        assert!(!keeps_row(status, now - 10 * 3_600, now, 3_600));
    }

    #[test]
    fn fresh_running_rows_survive_retention() {
        // An in-flight session must never be aged out while it keeps moving.
        let now = 2_000_000;
        let (status, _) = map_row_status("", now - 30, now);
        assert_eq!(status, TaskStatus::Running);
        assert!(keeps_row(status, 0, now, 3_600));
    }

    #[test]
    fn model_display_drops_account_prefix_but_keeps_the_name() {
        assert_eq!(
            short_model("account:zai-start-plan/GLM-5.3-Flash"),
            "GLM-5.3-Flash"
        );
        assert_eq!(short_model("glm-4.6"), "glm-4.6");
        assert_eq!(short_model("provider/"), "provider/");
        assert_eq!(short_model("  "), "");
    }

    #[test]
    fn pending_is_never_reported_as_waiting_input() {
        let (status, reason) = map_status("pending");
        assert_ne!(status, TaskStatus::WaitingInput);
        assert!(reason.contains("unmapped"));
    }

    #[test]
    fn ambiguous_or_step_statuses_are_not_final_task_completion() {
        for status in ["done", "complete", "step_completed", "tool_completed"] {
            assert_eq!(map_status(status).0, TaskStatus::Unknown, "{status}");
        }
    }

    #[test]
    fn workspace_deep_link_is_percent_encoded() {
        let link = format!(
            "zcode://workspace/open?path={}",
            percent_encode("/Users/me/My Project")
        );
        assert!(link.contains("My%20Project"));
    }

    #[test]
    fn stable_id_uses_task_id_not_title() {
        let id = format!("zcode:{}", "sess_abc");
        assert_eq!(id, "zcode:sess_abc");
        assert_ne!(id, "zcode:Some title");
    }

    #[test]
    fn retention_governs_ended_rows_including_the_newest_and_keeps_active_rows() {
        let now = 2_000_000;
        let window = 72 * 3_600;
        // Regression: there is no unconditional "newest 3 rows" fallback any
        // more. An ended row is dropped as soon as its `updated_at` is outside
        // the configured window, whatever its position in the index order.
        let fifteen_days_ago = now - 15 * 86_400;
        assert!(!keeps_row(
            TaskStatus::Completed,
            fifteen_days_ago,
            now,
            window
        ));
        assert!(!keeps_row(TaskStatus::Failed, fifteen_days_ago, now, window));
        // The inclusive boundary is kept.
        assert!(keeps_row(TaskStatus::Completed, now - window, now, window));
        // Active rows are never aged out.
        for status in [
            TaskStatus::Running,
            TaskStatus::WaitingInput,
            TaskStatus::WaitingApproval,
        ] {
            assert!(keeps_row(status, 0, now, window), "{status:?} must stay");
        }
        // Retention is injected, not the old hard-coded 24 h ZCode constant.
        let two_days_ago = now - 48 * 3_600;
        assert!(keeps_row(TaskStatus::Completed, two_days_ago, now, window));
        assert!(!keeps_row(
            TaskStatus::Completed,
            two_days_ago,
            now,
            24 * 3_600
        ));
    }
}
