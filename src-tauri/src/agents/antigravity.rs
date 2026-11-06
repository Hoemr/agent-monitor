//! Antigravity desktop and CLI local SQLite indices (read-only). Empty and
//! child sessions are omitted; completion requires a final FINISH or an idle,
//! DONE planner response with STOP_PATTERN and no tool calls.

use super::{retention_keeps, truncate_chars, ScanCtx, Source};
use crate::model::*;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};

// Values from the installed agy protobuf descriptors (2026-10-30).
const FINISH: i64 = 2;
const DONE: i64 = 3;

pub struct AntigravitySource {
    root: PathBuf,
    desktop_root: PathBuf,
    models: HashMap<PathBuf, (ModelStamp, Option<String>)>,
}

impl AntigravitySource {
    pub fn new() -> Self {
        let home = super::home_dir().unwrap_or_default();
        Self { root: home.join(".gemini/antigravity-cli"), desktop_root: home.join(".gemini/antigravity"), models: HashMap::new() }
    }
}

fn open_ro(path: &Path) -> rusqlite::Result<Connection> {
    // Closed agy databases can retain WAL mode without a shared-memory file.
    // Live WAL must be read normally so committed updates are included.
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let encoded = path.to_string_lossy().replace('\\', "/").replace('%', "%25").replace(' ', "%20")
        .replace('#', "%23").replace('?', "%3F");
    let mode = if wal.exists() { "mode=ro" } else { "mode=ro&immutable=1" };
    Connection::open_with_flags(format!("file:{encoded}?{mode}"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX)
}

fn conversation_path(root: &Path, id: &str) -> Option<PathBuf> {
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| root.join("conversations").join(format!("{id}.db")))
}

// Wire field numbers and enum values verified against the installed desktop
// language_server's exa.cortex_pb / exa.codeium_common_pb descriptors (2026-11-03).
#[derive(Debug)]
enum ProtoValue<'a> { Number(u64), Bytes(&'a [u8]), Other }

fn read_varint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *data.get(*offset)?;
        *offset += 1;
        if shift == 63 && byte > 1 { return None; }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 { return Some(value); }
    }
    None
}

fn proto_fields(data: &[u8]) -> Option<Vec<(u32, ProtoValue<'_>)>> {
    let mut offset = 0;
    let mut fields = Vec::new();
    while offset < data.len() {
        let key = read_varint(data, &mut offset)?;
        let number = u32::try_from(key >> 3).ok()?;
        if number == 0 { return None; }
        let value = match key & 7 {
            0 => ProtoValue::Number(read_varint(data, &mut offset)?),
            2 => {
                let len = usize::try_from(read_varint(data, &mut offset)?).ok()?;
                let end = offset.checked_add(len)?;
                let bytes = data.get(offset..end)?;
                offset = end;
                ProtoValue::Bytes(bytes)
            }
            wire @ (1 | 5) => {
                offset = offset.checked_add(if wire == 1 { 8 } else { 4 })?;
                if offset > data.len() { return None; }
                ProtoValue::Other
            }
            _ => return None,
        };
        fields.push((number, value));
    }
    Some(fields)
}

type ModelStamp = (Option<(std::time::SystemTime, u64)>, Option<(std::time::SystemTime, u64)>);

fn model_stamp(path: &Path) -> ModelStamp {
    let stamp = |p: &Path| p.metadata().ok().and_then(|m| Some((m.modified().ok()?, m.len())));
    (stamp(path), stamp(&PathBuf::from(format!("{}-wal", path.display()))))
}

fn proto_bytes<'a>(fields: &[(u32, ProtoValue<'a>)], tag: u32) -> Option<&'a [u8]> {
    fields.iter().rev().find_map(|(n, v)| match v {
        ProtoValue::Bytes(bytes) if *n == tag => Some(*bytes), _ => None,
    })
}

fn proto_number(fields: &[(u32, ProtoValue<'_>)], tag: u32) -> Option<u64> {
    fields.iter().rev().find_map(|(n, v)| match v {
        ProtoValue::Number(number) if *n == tag => Some(*number), _ => None,
    })
}

fn model_text(fields: &[(u32, ProtoValue<'_>)], tags: &[u32]) -> Option<String> {
    tags.iter().find_map(|tag| {
        let text = std::str::from_utf8(proto_bytes(fields, *tag)?).ok()?.trim();
        (!text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)).then(|| text.to_owned())
    })
}

/// CortexStepGeneratorMetadata.chat_model = field 1. Only the metadata
/// explicitly covering the parent step may supply its recorded model.
fn generator_model(data: &[u8], step_idx: u64) -> Option<String> {
    let fields = proto_fields(data)?;
    let mut covers_step = false;
    for (tag, value) in &fields {
        if *tag != 2 { continue; } // step_indices (packed or unpacked)
        match value {
            ProtoValue::Number(idx) => covers_step |= *idx == step_idx,
            ProtoValue::Bytes(indices) => {
                let mut offset = 0;
                while offset < indices.len() { covers_step |= read_varint(indices, &mut offset)? == step_idx; }
            }
            _ => return None,
        }
    }
    if !covers_step { return None; }
    let chat = proto_fields(proto_bytes(&fields, 1)?)?;
    // Names recorded by the generator, never a global model preference or an
    // obfuscated numeric enum. Prefer the UI label, including effort level.
    model_text(&chat, &[21, 22, 19])
}

fn latest_model(path: &Path, conversation: &str) -> Option<String> {
    let conn = open_ro(path).ok()?;
    let metadata: Option<Vec<u8>> = conn.query_row(
        "SELECT CASE WHEN length(metadata) <= 1048576 THEN metadata ELSE NULL END
         FROM steps WHERE step_type = 15 AND idx > coalesce(
            (SELECT max(idx) FROM steps WHERE step_type = 14), -1)
         ORDER BY idx DESC LIMIT 1", [], |r| r.get(0),
    ).ok()?;
    let metadata = proto_fields(metadata.as_deref()?)?;
    let source = proto_bytes(&metadata, 20).and_then(proto_fields);
    if let Some(source) = &source {
        if model_text(source, &[4]).as_deref() != Some(conversation) { return None; }
    }
    // ModelInfo is available in some versions instead of a generator name.
    if let Some(info) = proto_bytes(&metadata, 24).and_then(proto_fields) {
        if let Some(name) = model_text(&info, &[20, 8, 12]) { return Some(name); }
    }
    let source = source?;
    if model_text(&source, &[4]).as_deref() != Some(conversation) { return None; }
    let generator = i64::try_from(proto_number(&source, 3).unwrap_or(0)).ok()?;
    let step = proto_number(&source, 2).unwrap_or(0);
    let data: Option<Vec<u8>> = conn.query_row(
        "SELECT CASE WHEN length(data) <= 16777216 THEN data ELSE NULL END FROM gen_metadata WHERE idx = ?1",
        [generator], |r| r.get(0),
    ).ok()?;
    generator_model(data.as_deref()?, step)
}

#[derive(Default)]
struct Step {
    idx: i64,
    kind: i64,
    status: i64,
    final_response: bool,
    waiting: Option<TaskStatus>,
}

fn parse_step(idx: i64, kind: i64, status: i64, payload: &[u8]) -> Step {
    let mut step = Step { idx, kind, status, ..Default::default() };
    let Some(fields) = proto_fields(payload) else { return step; };
    for (number, value) in fields {
        let ProtoValue::Bytes(bytes) = value else { continue; };
        if kind == 15 && status == DONE && number == 20 {
            if let Some(response) = proto_fields(bytes) {
                let stop = response.iter().any(|(n, v)| matches!((n, v), (12, ProtoValue::Number(2))));
                let text = response.iter().any(|(n, v)| matches!((n, v), (1, ProtoValue::Bytes(b)) if !b.is_empty()));
                let tools = response.iter().any(|(n, _)| *n == 7);
                step.final_response = stop && text && !tools;
            }
        }
        // WAITING alone is ambiguous. Require an explicit ask/permission request.
        if status == 9 && number == 56 {
            if let Some(request) = proto_fields(bytes) {
                for (n, value) in request {
                    if matches!(value, ProtoValue::Bytes(_)) {
                        step.waiting = match n {
                            22 => Some(TaskStatus::WaitingInput),
                            21 => Some(TaskStatus::WaitingApproval),
                            _ => step.waiting,
                        };
                    }
                }
            }
        }
    }
    step
}

fn latest_step(path: &Path) -> Option<Step> {
    open_ro(path).ok()?.query_row(
        "SELECT idx, step_type, status,
         CASE WHEN length(step_payload) <= 8388608 THEN step_payload ELSE NULL END
         FROM steps ORDER BY idx DESC LIMIT 1",
        [], |r| {
            let payload: Option<Vec<u8>> = r.get(3)?;
            Ok(parse_step(r.get(0)?, r.get(1)?, r.get(2)?, payload.as_deref().unwrap_or_default()))
        },
    ).ok()
}

fn state_from_step(status: &str, killed: bool, step: Option<&Step>, alive: bool)
    -> (TaskStatus, Option<String>, String)
{
    let base = task_state(status, killed, step.map(|s| (s.idx, s.kind, s.status)), alive);
    if killed { return base; }
    if let Some(step) = step {
        if let Some(waiting) = step.waiting {
            return (waiting, None, "Antigravity WAITING step with explicit interaction request".into());
        }
        if status == "CASCADE_RUN_STATUS_IDLE" {
            if step.final_response {
                return (TaskStatus::Completed, Some(format!("response:{}", step.idx)),
                    "Antigravity idle + DONE planner response; STOP_PATTERN, no tool calls".into());
            }
            if matches!(step.status, 6 | 7 | 12) {
                return (TaskStatus::Failed, None, if step.status == 7 {
                    "Antigravity ERROR step".into()
                } else { "Antigravity cancelled/interrupted step".into() });
            }
        }
    }
    base
}

fn task_state(status: &str, killed: bool, step: Option<(i64, i64, i64)>, alive: bool)
    -> (TaskStatus, Option<String>, String)
{
    if killed {
        return (TaskStatus::Failed, None, "Antigravity conversation cancelled (killed)".into());
    }
    if matches!(status, "CASCADE_RUN_STATUS_RUNNING" | "CASCADE_RUN_STATUS_BUSY" | "CASCADE_RUN_STATUS_CANCELING") {
        return (if alive { TaskStatus::Running } else { TaskStatus::Unknown }, None,
            format!("{status}; host {}", if alive { "present" } else { "not running" }));
    }
    if status == "CASCADE_RUN_STATUS_IDLE" {
        if let Some((idx, FINISH, DONE)) = step {
            return (TaskStatus::Completed, Some(format!("finish:{idx}")), "Antigravity DONE FINISH step".into());
        }
        // An idle agent or a completed tool is not a completed task.
        return (TaskStatus::Idle, None, "Antigravity idle without a final FINISH step".into());
    }
    (TaskStatus::Unknown, None, format!("unmapped Antigravity status: {status}"))
}

impl Source for AntigravitySource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "antigravity".into(), name: "Antigravity".into(),
            source: format!("{}; {}", self.desktop_root.display(), self.root.display()),
            origin: DataOrigin::Index, max_level: JumpLevel::L1,
            notes: "Desktop + CLI SQLite summaries and explicit final responses; application activation only".into(),
        }
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let desktop_root = self.desktop_root.clone();
        let cli_root = self.root.clone();
        let mut tasks = self.scan_root(ctx, &desktop_root, true);
        tasks.extend(self.scan_root(ctx, &cli_root, false));
        let paths: HashSet<_> = tasks.iter().filter_map(|t|
            conversation_path(if t.host == "desktop" { &desktop_root } else { &cli_root }, t.session_id.as_deref()?)
        ).collect();
        self.models.retain(|path, _| paths.contains(path));
        tasks
    }

    fn invalidate(&mut self) { self.models.clear(); }
}

impl AntigravitySource {
    fn recorded_model(&mut self, path: &Path, conversation: &str) -> Option<String> {
        let stamp = model_stamp(path);
        if let Some((cached_stamp, model)) = self.models.get(path) {
            if *cached_stamp == stamp { return model.clone(); }
        }
        let model = latest_model(path, conversation);
        self.models.insert(path.to_path_buf(), (stamp, model.clone()));
        model
    }

    fn scan_root(&mut self, ctx: &ScanCtx, root: &Path, desktop: bool) -> Vec<TaskRecord> {
        let index = root.join("conversation_summaries.db");
        if !index.exists() { return Vec::new(); }
        let rows = (|| -> rusqlite::Result<Vec<(String, String, String, String, u64, bool)>> {
            let conn = open_ro(&index)?;
            let mut stmt = conn.prepare(
                "SELECT conversation_id,title,workspace_uris,status,
                 max(coalesce(unixepoch(last_modified_time),0),coalesce(unixepoch(last_user_input_time),0),0),killed
                 FROM conversation_summaries WHERE step_count > 0 AND parent_conversation_id = ''
                 ORDER BY last_modified_time DESC LIMIT 120")?;
            let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)?.max(0) as u64, r.get(5)?)))?;
            mapped.collect()
        })();
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => { eprintln!("{}: {error}", index.display()); return Vec::new(); }
        };
        let alive = ctx.processes.entries.iter().any(|p| {
            if desktop {
                p.command.contains("Antigravity.app/Contents/MacOS/Antigravity")
            } else {
                Path::new(p.command.split_whitespace().next().unwrap_or(""))
                    .file_name().is_some_and(|n| n == "agy")
            }
        }) || ctx.processes.has_executable(if desktop { "Antigravity" } else { "agy" });
        rows.into_iter().filter_map(|(id, title, workspaces, run_status, activity, killed)| {
            let path = conversation_path(root, &id)?;
            let step = latest_step(&path);
            let (status, completion_id, reason) = state_from_step(&run_status, killed, step.as_ref(), alive);
            let task_id = if desktop { format!("antigravity:desktop:{id}") } else { format!("antigravity:{id}") };
            if !ctx.pinned_ids.contains(&task_id) && !retention_keeps(status, activity, ctx.now_secs, ctx.retention_secs) {
                return None;
            }
            let workspace = serde_json::from_str::<Vec<String>>(&workspaces).ok()
                .and_then(|uris| uris.into_iter().find_map(|u| tauri::Url::parse(&u).ok()?.to_file_path().ok()))
                .map(|p| p.to_string_lossy().into_owned());
            let title = if title.trim().is_empty() {
                let base = workspace.as_deref().and_then(|w| Path::new(w).file_name()).map(|s| s.to_string_lossy()).unwrap_or_default();
                format!("Antigravity ({base})")
            } else { truncate_chars(title.trim(), 120) };
            let model = self.recorded_model(&path, &id);
            Some(TaskRecord {
                id: task_id, agent_key: "antigravity".into(),
                agent_name: if desktop { "Antigravity" } else { "Antigravity CLI" }.into(),
                host: if desktop { "desktop" } else { "terminal" }.into(), session_id: Some(id), task_id: None, title, status,
                completion_id, completion: (status == TaskStatus::Completed).then_some(if step.as_ref().is_some_and(|s| s.final_response) {
                    CompletionEvidence::AssistantEndTurn
                } else { CompletionEvidence::TaskEnd }),
                status_reason: reason, last_activity_secs: activity, collected_at_secs: ctx.now_secs,
                summary: None, model, origin: DataOrigin::Index,
                target: JumpTarget {
                    app_name: Some(if desktop { "Antigravity" } else { "Terminal" }.into()),
                    bundle_id: Some(if desktop { "com.google.antigravity" } else { "com.apple.Terminal" }.into()),
                    terminal_host: (!desktop).then(|| "Terminal".into()), workspace_path: workspace, ..Default::default()
                },
                target_validity: TargetValidity::Unconfirmed, invalid_reason: None,
                capability: JumpCapability {
                    max_level: JumpLevel::L1, requires: vec![PermissionKind::Automation],
                    notes: "Application activation only; no verified session navigation".into(),
                },
                level: JumpLevel::L1, pinned: false,
            })
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_scan_skips_empty_and_child_sessions_and_observes_live_wal_updates() {
        let root = std::env::temp_dir().join(format!("am-agy-{}-{}", std::process::id(), now_millis()));
        std::fs::create_dir_all(root.join("conversations")).unwrap();
        let db = Connection::open(root.join("conversation_summaries.db")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE conversation_summaries(conversation_id TEXT, title TEXT, workspace_uris TEXT,
                status TEXT, last_modified_time TEXT, last_user_input_time TEXT, killed INTEGER,
                step_count INTEGER, parent_conversation_id TEXT);
            INSERT INTO conversation_summaries VALUES('empty','','[]','CASCADE_RUN_STATUS_IDLE',datetime('now'),datetime('now'),0,0,'');
            INSERT INTO conversation_summaries VALUES('child','Child','[]','CASCADE_RUN_STATUS_IDLE',datetime('now'),datetime('now'),0,1,'parent');").unwrap();
        let steps = Connection::open(root.join("conversations/parent.db")).unwrap();
        steps.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE steps(idx INTEGER PRIMARY KEY, step_type INTEGER, status INTEGER, step_payload BLOB);
            INSERT INTO steps(idx,step_type,status) VALUES(1,6,3);").unwrap();
        let ctx = ScanCtx::new(now_secs(), std::sync::Arc::new(super::super::ProcessTable::default()), 3600);
        let mut source = AntigravitySource { root: root.clone(), desktop_root: root.join("desktop"), models: HashMap::new() };
        assert!(source.scan(&ctx).is_empty());
        db.execute_batch("INSERT INTO conversation_summaries VALUES('parent','Task','[\"file:///tmp/My%20Project\"]',
            'CASCADE_RUN_STATUS_IDLE',datetime('now'),datetime('now'),0,1,'')").unwrap();
        let workspace = root.join("My Project");
        let uri = tauri::Url::from_directory_path(&workspace).unwrap().to_string();
        db.execute("UPDATE conversation_summaries SET workspace_uris=?1 WHERE conversation_id='parent'",
            [serde_json::to_string(&vec![uri]).unwrap()]).unwrap();
        let tasks = source.scan(&ctx);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Idle);
        assert_eq!(Path::new(tasks[0].target.workspace_path.as_deref().unwrap()), workspace);
        steps.execute("INSERT INTO steps(idx,step_type,status) VALUES(2,?1,?2)", [FINISH, DONE]).unwrap();
        let completed = source.scan(&ctx);
        assert_eq!(completed[0].status, TaskStatus::Completed);
        assert_eq!(completed[0].completion_id.as_deref(), Some("finish:2"));
        assert_eq!(completed[0].completion, Some(CompletionEvidence::TaskEnd));
        db.execute_batch("UPDATE conversation_summaries SET last_modified_time=datetime('now','+1 second') WHERE conversation_id='parent'").unwrap();
        assert_eq!(source.scan(&ctx)[0].completion_id, completed[0].completion_id);
        steps.execute_batch("INSERT INTO steps(idx,step_type,status) VALUES(3,14,3)").unwrap();
        db.execute_batch("UPDATE conversation_summaries SET status='CASCADE_RUN_STATUS_RUNNING' WHERE conversation_id='parent'").unwrap();
        let mut live = ctx.clone();
        live.processes = std::sync::Arc::new(super::super::ProcessTable {
            entries: vec![super::super::ProcInfo { pid: 1, command: "/Users/me/.local/bin/agy".into() }], sampled_at_ms: now_millis()
        });
        let running = source.scan(&live);
        assert_eq!(running[0].status, TaskStatus::Running);
        assert!(running[0].completion_id.is_none());
        assert!(open_ro(&root.join("conversation_summaries.db")).unwrap().execute("DELETE FROM conversation_summaries", []).is_err());
        drop(steps);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_and_completed_tool_are_not_task_completion() {
        for step in [None, Some((10, 6, DONE)), Some((10, FINISH, 2))] {
            assert_eq!(task_state("CASCADE_RUN_STATUS_IDLE", false, step, false).0, TaskStatus::Idle);
        }
        let done = task_state("CASCADE_RUN_STATUS_IDLE", false, Some((10, FINISH, DONE)), false);
        assert_eq!(done.0, TaskStatus::Completed);
        assert_eq!(done.1.as_deref(), Some("finish:10"));
        assert_ne!(done.1, task_state("CASCADE_RUN_STATUS_IDLE", false, Some((20, FINISH, DONE)), false).1);
        assert_eq!(task_state("CASCADE_RUN_STATUS_RUNNING", false, Some((10, FINISH, DONE)), true).0, TaskStatus::Running);
        assert_eq!(task_state("CASCADE_RUN_STATUS_RUNNING", false, None, false).0, TaskStatus::Unknown);
        assert_eq!(task_state("CASCADE_RUN_STATUS_IDLE", true, Some((10, FINISH, DONE)), false).0, TaskStatus::Failed);
    }

    fn varint(mut n: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while n >= 128 { out.push((n as u8 & 127) | 128); n >>= 7; }
        out.push(n as u8);
        out
    }

    fn message(number: u64, bytes: &[u8]) -> Vec<u8> {
        let mut out = varint((number << 3) | 2);
        out.extend(varint(bytes.len() as u64));
        out.extend(bytes);
        out
    }

    fn response(stop: u8, tools: bool) -> Vec<u8> {
        let mut body = message(1, b"done");
        body.extend([0x60, stop]); // planner stop_reason = field 12
        if tools { body.extend(message(7, b"tool")); }
        message(20, &body)
    }

    #[test]
    fn desktop_final_response_requires_explicit_stop_and_idle_parent() {
        for (kind, status, stop, tools) in [(15, 2, 2, false), (15, DONE, 10, true),
            (15, DONE, 3, false), (6, DONE, 2, false), (15, DONE, 2, true)] {
            let step = parse_step(10, kind, status, &response(stop, tools));
            assert!(!step.final_response);
            assert_ne!(state_from_step("CASCADE_RUN_STATUS_IDLE", false, Some(&step), false).0,
                TaskStatus::Completed);
        }
        let step = parse_step(10, 15, DONE, &response(2, false));
        let completed = state_from_step("CASCADE_RUN_STATUS_IDLE", false, Some(&step), false);
        assert_eq!(completed.0, TaskStatus::Completed);
        assert_eq!(completed.1.as_deref(), Some("response:10"));
        assert_eq!(state_from_step("CASCADE_RUN_STATUS_RUNNING", false, Some(&step), true).0,
            TaskStatus::Running);
        assert_eq!(state_from_step("CASCADE_RUN_STATUS_RUNNING", false, Some(&step), false).0,
            TaskStatus::Unknown);
    }

    #[test]
    fn desktop_interactions_require_waiting_and_a_typed_request() {
        for (field, status) in [(22, TaskStatus::WaitingInput), (21, TaskStatus::WaitingApproval)] {
            let payload = message(56, &message(field, b"request"));
            let step = parse_step(5, 132, 9, &payload);
            assert_eq!(state_from_step("CASCADE_RUN_STATUS_RUNNING", false, Some(&step), true).0, status);
            let answered = parse_step(5, 132, DONE, &payload);
            assert!(answered.waiting.is_none());
        }
        let untyped = parse_step(5, 132, 9, &message(56, &message(99, b"unknown")));
        assert!(untyped.waiting.is_none());
    }

    #[test]
    fn malformed_proto_and_nonfinal_text_never_create_completion() {
        for data in [vec![0], vec![0x80; 12], vec![0xa2, 1, 127],
            message(20, b"malformed"), message(20, &message(1, b"ordinary text"))] {
            assert!(!parse_step(1, 15, DONE, &data).final_response);
        }
        let overflow = [vec![0xa2, 1], vec![0xff; 9], vec![2]].concat();
        assert!(!parse_step(1, 15, DONE, &overflow).final_response);
    }

    #[test]
    fn desktop_and_cli_keep_distinct_ids_and_read_live_updates() {
        let base = std::env::temp_dir().join(format!("am-agy-desktop-{}-{}", std::process::id(), now_millis()));
        let mut source = AntigravitySource { root: base.join("cli"), desktop_root: base.join("desktop"), models: HashMap::new() };
        let mut writers = Vec::new();
        for root in [&source.root, &source.desktop_root] {
            std::fs::create_dir_all(root.join("conversations")).unwrap();
            let db = Connection::open(root.join("conversation_summaries.db")).unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
                CREATE TABLE conversation_summaries(conversation_id TEXT, title TEXT, workspace_uris TEXT,
                    status TEXT, last_modified_time TEXT, last_user_input_time TEXT, killed INTEGER,
                    step_count INTEGER, parent_conversation_id TEXT);
                INSERT INTO conversation_summaries VALUES('same-id','Task','[]','CASCADE_RUN_STATUS_IDLE',
                    datetime('now'),datetime('now'),0,1,'');").unwrap();
            let steps = Connection::open(root.join("conversations/same-id.db")).unwrap();
            steps.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
                CREATE TABLE steps(idx INTEGER PRIMARY KEY, step_type INTEGER, status INTEGER, step_payload BLOB);").unwrap();
            steps.execute("INSERT INTO steps VALUES(0,15,3,?1)", [response(2, false)]).unwrap();
            writers.push((db, steps));
        }
        let mut ctx = ScanCtx::new(now_secs(), std::sync::Arc::new(super::super::ProcessTable::default()), 3600);
        let first = source.scan(&ctx);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].id, "antigravity:desktop:same-id");
        assert_eq!(first[1].id, "antigravity:same-id");
        assert_eq!(first[0].host, "desktop");
        assert_eq!(first[0].target.bundle_id.as_deref(), Some("com.google.antigravity"));
        assert!(first[0].target.terminal_host.is_none());
        assert_eq!(first[1].target.bundle_id.as_deref(), Some("com.apple.Terminal"));
        assert!(first.iter().all(|t| t.status == TaskStatus::Completed
            && t.completion == Some(CompletionEvidence::AssistantEndTurn)));
        writers[1].0.execute("UPDATE conversation_summaries SET last_modified_time=datetime('now','+1 second')", []).unwrap();
        assert_eq!(source.scan(&ctx)[0].completion_id, first[0].completion_id);
        writers[1].0.execute("UPDATE conversation_summaries SET status='CASCADE_RUN_STATUS_RUNNING'", []).unwrap();
        ctx.processes = std::sync::Arc::new(super::super::ProcessTable {
            entries: vec![super::super::ProcInfo { pid: 1, command: "/Applications/Antigravity.app/Contents/MacOS/Antigravity".into() }], sampled_at_ms: now_millis()
        });
        let running = source.scan(&ctx);
        assert_eq!(running[0].status, TaskStatus::Running);
        assert!(running[0].completion_id.is_none());
        assert_eq!(running[1].status, TaskStatus::Completed);
        writers[1].1.execute("INSERT INTO steps VALUES(1,15,3,?1)", [response(2, false)]).unwrap();
        writers[1].0.execute("UPDATE conversation_summaries SET status='CASCADE_RUN_STATUS_IDLE'", []).unwrap();
        assert_eq!(source.scan(&ctx)[0].completion_id.as_deref(), Some("response:1"));
        drop(writers);
        // Closed WAL databases must also be readable without a live writer.
        assert_eq!(source.scan(&ctx).len(), 2);
        std::fs::remove_dir_all(base).unwrap();
    }

    fn number(tag: u64, value: u64) -> Vec<u8> {
        [varint(tag << 3), varint(value)].concat()
    }

    fn model_generator(step: u64, name: &str) -> Vec<u8> {
        let chat = [message(19, b"model-a-raw"), message(21, name.as_bytes())].concat();
        [message(2, &varint(step)), message(1, &chat)].concat()
    }

    fn model_reference(session: &str, step: u64, generator: u64) -> Vec<u8> {
        message(20, &[number(2, step), number(3, generator), message(4, session.as_bytes())].concat())
    }

    #[test]
    fn generator_names_use_recorded_labels_or_actual_ids_and_verify_step_membership() {
        assert_eq!(generator_model(&model_generator(1, "Model Alpha (High)"), 1).as_deref(), Some("Model Alpha (High)"));
        assert!(generator_model(&model_generator(1, "Model Alpha"), 2).is_none());
        let fallback = [number(2, 1), message(1, &message(19, b"model-actual-v2"))].concat();
        assert_eq!(generator_model(&fallback, 1).as_deref(), Some("model-actual-v2"));
        let packed = [message(2, &[varint(1), varint(300)].concat()), message(1, &message(22, b"provider/model-v3"))].concat();
        assert_eq!(generator_model(&packed, 300).as_deref(), Some("provider/model-v3"));
        assert!(generator_model(&message(2, &[0x80]), 1).is_none());
        let invalid = [number(2, 1), message(1, &message(21, &[0xff]))].concat();
        assert!(generator_model(&invalid, 1).is_none());
        let empty = [number(2, 1), message(1, &message(21, b"  "))].concat();
        assert!(generator_model(&empty, 1).is_none());
    }

    #[test]
    fn current_parent_turn_model_tracks_live_wal_changes_and_never_inherits_old_turn() {
        let root = std::env::temp_dir().join(format!("am-agy-model-{}-{}", std::process::id(), now_millis()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE steps(idx INTEGER PRIMARY KEY, step_type INTEGER, metadata BLOB);
            CREATE TABLE gen_metadata(idx INTEGER PRIMARY KEY, data BLOB);
            INSERT INTO steps VALUES(0,14,NULL);").unwrap();
        db.execute("INSERT INTO steps VALUES(1,15,?1)", [model_reference("parent", 1, 0)]).unwrap();
        db.execute("INSERT INTO gen_metadata VALUES(0,?1)", [model_generator(1, "Model Alpha (High)" )]).unwrap();
        let mut source = AntigravitySource { root: root.clone(), desktop_root: root.clone(), models: HashMap::new() };
        assert_eq!(source.recorded_model(&path, "parent").as_deref(), Some("Model Alpha (High)"));
        // A newer metadata row for another step is not the parent reply's model.
        db.execute("INSERT INTO gen_metadata VALUES(1,?1)", [model_generator(999, "Other Model")]).unwrap();
        db.execute_batch("INSERT INTO steps VALUES(2,132,NULL)").unwrap();
        assert_eq!(source.recorded_model(&path, "parent").as_deref(), Some("Model Alpha (High)"));
        db.execute("UPDATE gen_metadata SET data=?1 WHERE idx=0", [model_generator(1, "Model Beta (Low)")]).unwrap();
        assert_eq!(source.recorded_model(&path, "parent").as_deref(), Some("Model Beta (Low)"));
        db.execute_batch("INSERT INTO steps VALUES(3,14,NULL)").unwrap();
        assert!(source.recorded_model(&path, "parent").is_none());
        db.execute("INSERT INTO steps VALUES(4,15,?1)", [model_reference("parent", 4, 2)]).unwrap();
        assert!(source.recorded_model(&path, "parent").is_none()); // generator has not arrived
        db.execute("INSERT INTO gen_metadata VALUES(2,?1)", [model_generator(4, "Model Gamma")]).unwrap();
        assert_eq!(source.recorded_model(&path, "parent").as_deref(), Some("Model Gamma"));
        source.invalidate(); assert!(source.models.is_empty());
        drop(db);
        assert_eq!(latest_model(&path, "parent").as_deref(), Some("Model Gamma"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_info_is_supported_but_foreign_cascades_and_malformed_metadata_are_rejected() {
        let root = std::env::temp_dir().join(format!("am-agy-model-info-{}-{}", std::process::id(), now_millis()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE steps(idx INTEGER PRIMARY KEY, step_type INTEGER, metadata BLOB);
            INSERT INTO steps VALUES(0,14,NULL)").unwrap();
        let info = message(24, &message(20, b"Recorded Model Name"));
        db.execute("INSERT INTO steps VALUES(1,15,?1)", [&info]).unwrap();
        assert_eq!(latest_model(&path, "parent").as_deref(), Some("Recorded Model Name"));
        let foreign = [info, model_reference("child", 1, 0)].concat();
        db.execute("UPDATE steps SET metadata=?1 WHERE idx=1", [foreign]).unwrap();
        assert!(latest_model(&path, "parent").is_none());
        db.execute("UPDATE steps SET metadata=?1 WHERE idx=1", [vec![0x80;12]]).unwrap();
        assert!(latest_model(&path, "parent").is_none());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn conversation_ids_cannot_escape_the_data_directory() {
        for id in ["", "../other", "/tmp/other", "a/b"] { assert!(conversation_path(Path::new("/data"), id).is_none()); }
        assert_eq!(conversation_path(Path::new("/data"), "abc-123").unwrap(), Path::new("/data/conversations/abc-123.db"));
    }
}
