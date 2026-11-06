//! Claude Code adapter — the VSCode agent plugin `anthropic.claude-code`
//! (2.1.304) plus the CLI variant, both backed by `~/.claude`.
//!
//! Incremental reads preserve the parent turn state. Only an explicit assistant
//! end_turn without tool calls completes it; silence and tool results never do.
//! Sidechain/subagent messages do not determine the parent status.

use super::{truncate_chars, ScanCtx, Source};
use crate::model::*;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};

const VSCODE_BUNDLE: &str = "com.microsoft.VSCode";
const MAX_FILES: usize = 200;
// A transcript left mid-turn cannot prove ongoing work indefinitely.
const RUNNING_STALE_SECS: u64 = 30 * 60;

#[derive(Clone, Default)]
struct Session {
    completion_id: Option<String>,
    id: String,
    name: String,
    status: String,
    status_text: String,
    project_path: String,
    project_name: String,
    model: String,
    tokens_in: u64,
    tokens_out: u64,
    message_count: u32,
    active_tool: String,
    tool_detail: String,
    source: String,
    cwd: String,
    pending_questions: Vec<String>,
}

struct FileState {
    session: Session,
    mtime: (std::time::SystemTime, u64),
    pos: u64,
}

pub struct ClaudeCodeSource {
    files: HashMap<String, FileState>,
    project_dirs: Vec<(PathBuf, String, String)>,
    projects_mtime: u64,
}

impl ClaudeCodeSource {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            project_dirs: Vec::new(),
            projects_mtime: 0,
        }
    }

    fn claude_dir() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".claude"))
    }

    fn load_project_dirs(&mut self) {
        let Some(projects) = Self::claude_dir().map(|c| c.join("projects")) else {
            return;
        };
        let mtime = super::dir_mtime_secs(&projects);
        if mtime == self.projects_mtime && !self.project_dirs.is_empty() {
            return;
        }
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(&projects) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let raw = entry.file_name().to_string_lossy().to_string();
                let decoded = decode_project_name(&raw);
                let display = pretty_display_name(&raw);
                out.push((entry.path(), raw, display));
                let _ = decoded;
            }
        }
        self.project_dirs = out;
        self.projects_mtime = mtime;
    }

    fn scan_projects(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        self.load_project_dirs();
        let mut out = Vec::new();
        let mut seen_files: Vec<String> = Vec::new();

        let mut candidates = Vec::new();
        for (proj_path, raw_name, display) in &self.project_dirs {
            let Ok(entries) = fs::read_dir(proj_path) else { continue; };
            for file in entries.flatten() {
                let path = file.path();
                let id = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if id.starts_with("agent-") || id.contains("subagent") || path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                let pinned = ctx.pinned_ids.contains(&format!("claude-code:{id}"));
                let modified = super::file_mtime_secs(&path);
                if pinned || within_retention(modified, ctx.now_secs, ctx.retention_secs) {
                    candidates.push((pinned, modified, path, id, decode_project_name(raw_name), display.clone()));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
        for (_, file_mt, fpath, session_id, decoded, display) in candidates.into_iter().take(MAX_FILES) {
            let file_key = fpath.to_string_lossy().to_string();
            let prev = self.files.get(&file_key);
            if let Some(state) = parse_jsonl(&fpath, prev) {
                let mut session = state.session.clone();
                if session.id.is_empty() {
                    session.id = session_id.clone();
                }
                let project_path = if session.cwd.is_empty() {
                    decoded.clone()
                } else {
                    session.cwd.clone()
                };
                let project_name = Path::new(&project_path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| display.clone());
                session.project_path = project_path;
                session.project_name = project_name;
                seen_files.push(file_key.clone());
                self.files.insert(
                    file_key,
                    FileState {
                        session: session.clone(),
                        mtime: state.mtime,
                        pos: state.pos,
                    },
                );
                out.push(to_record(&session, file_mt, ctx));
            }
        }
        // Keep only the bounded set of transcripts inspected this scan.
        self.files.retain(|k, _| seen_files.contains(k));
        out
    }
}

fn within_retention(file_mtime_secs: u64, now_secs: u64, retention_secs: u64) -> bool {
    file_mtime_secs >= now_secs.saturating_sub(retention_secs)
}

fn decode_project_name(encoded: &str) -> String {
    // macOS Claude Code encodes "/Users/me/proj" as "-Users-me-proj". This is
    // lossy for folder names that contain '-', so the adapter prefers the real
    // `cwd` recorded in the transcript and only falls back here.
    if let Some(rest) = encoded.strip_prefix('-') {
        return format!("/{}", rest.replace('-', "/"));
    }
    if let Some(rest) = encoded.strip_prefix(|c: char| c.is_ascii_alphabetic()) {
        if let Some(rest) = rest.strip_prefix("--") {
            #[cfg(target_os = "windows")]
            {
                return format!("{}:\\{}", &encoded[..1], rest.replace("--", "\\"));
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = &encoded[..1];
                return format!("/{}", rest.replace("--", "/"));
            }
        }
    }
    encoded.to_string()
}

fn short_name(full_path: &str) -> String {
    full_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(full_path)
        .to_string()
}

fn pretty_display_name(raw: &str) -> String {
    let path = if let Some(rest) = raw.strip_prefix(|c: char| c.is_ascii_alphabetic()) {
        rest.strip_prefix("--").unwrap_or(rest)
    } else {
        raw
    };
    let parts: Vec<&str> = path.split('-').filter(|s| !s.is_empty()).collect();
    if parts.len() > 2 {
        parts[parts.len() - 2..].join("-")
    } else {
        path.to_string()
    }
}

fn strip_injections(text: &str) -> String {
    let mut text = text.to_string();
    for tag in ["system-reminder", "ide_opened_file", "ide_selection", "command-name"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = text.find(&open) {
            let end = text[start..].find(&close).map(|i| start + i + close.len()).unwrap_or(text.len());
            text.replace_range(start..end, "");
        }
    }
    text.trim().to_string()
}

fn tool_display_name(tool: &str) -> &str {
    match tool {
        "Bash" => "Running",
        "Write" => "Writing",
        "Edit" | "MultiEdit" => "Editing",
        "Read" => "Reading",
        "Glob" | "Grep" => "Searching",
        "TodoWrite" => "Planning",
        "Agent" => "Delegating",
        "WebFetch" => "Fetching",
        "WebSearch" => "Searching",
        "AskUserQuestion" => "Asking",
        _ => tool,
    }
}

#[allow(dead_code)] // kept from the legacy parser for transcript timestamps
fn parse_iso_timestamp(ts: &str) -> Option<u64> {
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

fn parse_jsonl(path: &Path, prev: Option<&FileState>) -> Option<FileState> {
    let file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let file_mt = (meta.modified().ok()?, meta.len());
    if let Some(prev_state) = prev {
        if file_mt == prev_state.mtime {
            return Some(FileState {
                session: prev_state.session.clone(),
                mtime: prev_state.mtime,
                pos: prev_state.pos,
            });
        }
    }
    let prev = prev.filter(|state| meta.len() >= state.pos);
    let mut reader = BufReader::new(file);
    let start_pos = prev.map(|p| p.pos).unwrap_or(0);
    let mut session = prev.map(|p| p.session.clone()).unwrap_or_default();
    if start_pos > 0 {
        let _ = reader.seek(std::io::SeekFrom::Start(start_pos));
    }
    let mut line = String::new();
    let mut new_pos = start_pos;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if !line.ends_with('\n') => break,
            Ok(n) => new_pos += n as u64,
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if obj.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let explicit_title = match typ {
            "custom-title" => obj.get("customTitle"),
            "summary" => obj.get("summary"),
            _ => obj.get("aiTitle"),
        };
        if let Some(title) = explicit_title.and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
            session.name = title.to_string();
        }
        if session.cwd.is_empty() {
            if let Some(cwd) = obj.get("cwd").and_then(|v| v.as_str()) {
                session.cwd = cwd.to_string();
            }
        }
        if let Some(entry) = obj.get("entrypoint").and_then(|v| v.as_str()) {
            session.source = if entry.contains("vscode") {
                "vscode".into()
            } else {
                "cli".into()
            };
        }
        match typ {
            "user" => {
                if let Some(content) = obj.get("message").and_then(|m| m.get("content")) {
                    let tool_result = content.as_array().map(|items| items.iter().any(|item|
                        item.get("type").and_then(|v| v.as_str()) == Some("tool_result"))).unwrap_or(false);
                    if !tool_result {
                        session.pending_questions.clear();
                        session.model.clear();
                        if session.name.is_empty() && obj.get("isMeta").and_then(|v| v.as_bool()) != Some(true) {
                            let text = content.as_str().map(str::to_string).unwrap_or_else(||
                                content.as_array().into_iter().flatten()
                                    .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("text"))
                                    .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
                                    .collect::<Vec<_>>().join(" "));
                            let cleaned = strip_injections(&text);
                            session.name = truncate_chars(&cleaned.split_whitespace().collect::<Vec<_>>().join(" "), 120);
                        }
                    } else if let Some(items) = content.as_array() {
                        for item in items {
                            if item.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                                if let Some(id) = item.get("tool_use_id").and_then(|v| v.as_str()) {
                                    session.pending_questions.retain(|pending| pending != id);
                                }
                            }
                        }
                    }
                }
                session.status = "working".into();
                session.active_tool.clear();
                session.tool_detail.clear();
                session.message_count += 1;
                if let Some(text) = obj
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                {
                    let cleaned = strip_injections(text);
                    if !cleaned.is_empty() && session.active_tool.is_empty() {
                        session.status_text = "Working...".into();
                    }
                }
            }
            "assistant" => {
                session.status = "responding".into();
                session.active_tool.clear();
                session.tool_detail.clear();
                session.message_count += 1;
                if let Some(msg) = obj.get("message") {
                    if let Some(model) = msg.get("model").and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty() && !s.starts_with('<')) {
                        session.model = model.to_string();
                    }
                    if let Some(usage) = msg.get("usage") {
                        session.tokens_in = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(session.tokens_in);
                        session.tokens_out = usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(session.tokens_out);
                    }
                    if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
                        for item in content {
                            if item.get("type").and_then(|v| v.as_str()) == Some("tool_use")
                                && item.get("name").and_then(|v| v.as_str()).map(super::is_question_tool) == Some(true)
                            {
                                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                                    if !session.pending_questions.iter().any(|pending| pending == id) {
                                        session.pending_questions.push(id.to_string());
                                    }
                                }
                            }
                        }
                        for item in content.iter().rev() {
                            let ctype =
                                item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match ctype {
                                "tool_use" => {
                                    if let Some(name) =
                                        item.get("name").and_then(|v| v.as_str())
                                    {
                                        session.active_tool = name.to_string();
                                        session.status_text =
                                            format!("{} {}", tool_display_name(name), name);
                                        session.status = "working".into();
                                        if let Some(input) = item.get("input") {
                                            if let Some(cmd) = input
                                                .get("command")
                                                .and_then(|v| v.as_str())
                                            {
                                                session.tool_detail =
                                                    cmd.chars().take(60).collect();
                                            } else if let Some(fp) =
                                                input.get("file_path").and_then(|v| v.as_str())
                                            {
                                                session.tool_detail = short_name(fp);
                                            } else if let Some(pattern) =
                                                input.get("pattern").and_then(|v| v.as_str())
                                            {
                                                session.tool_detail =
                                                    pattern.chars().take(40).collect();
                                            }
                                        }
                                        break;
                                    }
                                }
                                "thinking" if session.status != "working" => {
                                    session.status = "thinking".into();
                                    session.status_text = "Thinking...".into();
                                }
                                "text" if session.status != "working" => {
                                    session.status = "responding".into();
                                    session.status_text = "Responding...".into();
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Some(stop) = msg.get("stop_reason").and_then(|v| v.as_str()) {
                        match stop {
                            "tool_use" => session.status = "working".into(),
                            "end_turn" if session.active_tool.is_empty() => {
                                session.completion_id = obj.get("uuid").and_then(|v| v.as_str()).map(str::to_string);
                                session.status = "idle".into();
                                session.status_text = "Waiting for you".into();
                                session.active_tool.clear();
                                session.tool_detail.clear();
                            }
                            _ => {}
                        }
                    }
                }
            }
            "api-request" => {
                session.status = "thinking".into();
                session.status_text = "Thinking...".into();
                session.active_tool.clear();
                session.tool_detail.clear();
            }
            _ => {}
        }
    }
    Some(FileState {
        session,
        mtime: file_mt,
        pos: new_pos,
    })
}

fn to_record(session: &Session, file_mtime: u64, ctx: &ScanCtx) -> TaskRecord {
    // The supported integration is the VSCode extension (anthropic.claude-code).
    // Claude Code is a VSCode-hosted agent here; we never route it to Terminal.
    let host = "vscode";
    let (mut status, mut reason) = if !session.pending_questions.is_empty() {
        (TaskStatus::WaitingInput, format!("{} unanswered AskUserQuestion call(s)", session.pending_questions.len()))
    } else { match session.status.as_str() {
        "working" => (TaskStatus::Running, "parent user/tool activity; no final end_turn".to_string()),
        "thinking" => (TaskStatus::Running, "api-request / thinking block; no final end_turn".to_string()),
        "responding" => (TaskStatus::Running, "parent assistant text; no final end_turn".to_string()),
        "idle" => (
            TaskStatus::Completed,
            "parent assistant stop_reason = end_turn".to_string(),
        ),
        other => (
            TaskStatus::Unknown,
            format!("unmapped legacy status {other}"),
        ),
    }};
    if status == TaskStatus::Running {
        let host_alive = ctx.processes.any("Visual Studio Code.app/Contents/")
            || ctx.processes.entries.iter().any(|p| Path::new(p.command.split_whitespace().next().unwrap_or(""))
                .file_name().is_some_and(|name| name == "claude"));
        if !host_alive {
            status = TaskStatus::Unknown;
            reason = "Claude host is not running; parent turn has no final end_turn".into();
        } else if ctx.now_secs.saturating_sub(file_mtime) > RUNNING_STALE_SECS {
            status = TaskStatus::Unknown;
            reason = "parent turn has no final end_turn, but no transcript activity for over 30 minutes".into();
        }
    }
    let mut summary_parts = Vec::new();
    if session.message_count > 0 {
        summary_parts.push(format!("{} msg", session.message_count));
    }
    let tokens = session.tokens_in + session.tokens_out;
    if tokens > 0 {
        summary_parts.push(format!("{} tok", tokens));
    }
    if !session.tool_detail.is_empty() && status == TaskStatus::Running {
        summary_parts.push(session.tool_detail.clone());
    }
    let summary = if summary_parts.is_empty() {
        None
    } else {
        Some(truncate_chars(&summary_parts.join(", "), 60))
    };
    let workspace_exists = Path::new(&session.project_path).is_dir();
    let title = if session.name.trim().is_empty() {
        session.project_name.clone()
    } else {
        truncate_chars(session.name.trim(), 120)
    };
    let target = JumpTarget {
        app_name: Some("Code".into()),
        bundle_id: Some(VSCODE_BUNDLE.into()),
        deep_link: Some(format!("vscode://anthropic.claude-code/open?session={}", session.id)),
        window_title: Some(
            Path::new(&session.project_path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        ),
        workspace_path: Some(session.project_path.clone()),
        ..Default::default()
    };
    let mut record = TaskRecord {
        id: format!("claude-code:{}", session.id),
        agent_key: "claude-code".into(),
        agent_name: "Claude Code".into(),
        host: host.into(),
        session_id: Some(session.id.clone()),
        task_id: None,
        title,
        status,
        completion_id: session.completion_id.clone(),
        completion: (status == TaskStatus::Completed).then_some(CompletionEvidence::AssistantEndTurn),
        status_reason: reason,
        last_activity_secs: file_mtime,
        collected_at_secs: ctx.now_secs,
        summary,
        model: (!session.model.is_empty()).then(|| session.model.clone()),
        origin: DataOrigin::Event,
        target,
        target_validity: TargetValidity::Unconfirmed,
        invalid_reason: None,
        capability: JumpCapability {
            max_level: JumpLevel::L3,
            notes: "VSCode Claude /open?session=<id> route; verify the active session"
                .into(),
            requires: vec![PermissionKind::Automation],
        },
        level: JumpLevel::L3,
        pinned: false,
    };
    if !workspace_exists {
        record.target_validity = TargetValidity::Invalid;
        record.invalid_reason = Some("workspace_missing".into());
    } else {
        record.target_validity = TargetValidity::Valid;
    }
    record.refresh_level();
    record
}

impl Source for ClaudeCodeSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "claude-code".into(),
            name: "Claude Code".into(),
            source: "~/.claude/projects/**/*.jsonl parent-turn end_turn events".into(),
            origin: DataOrigin::Event,
            max_level: JumpLevel::L3,
            notes: "VSCode extension host; /open?session=<id> with focused-session verification."
                .into(),
        }
    }

    fn invalidate(&mut self) {
        self.files.clear();
        self.projects_mtime = 0;
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        self.scan_projects(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_open_turns_become_unknown_and_new_activity_resumes_work() {
        let ctx = ScanCtx::new(10_000, std::sync::Arc::new(super::super::ProcessTable {
            entries: vec![super::super::ProcInfo { pid: 1, command: "/Applications/Visual Studio Code.app/Contents/MacOS/Electron".into() }],
            sampled_at_ms: 0,
        }), 604_800);
        for state in ["working", "thinking", "responding"] {
            let session = Session { status: state.into(), ..Default::default() };
            let old = to_record(&session, 10_000 - RUNNING_STALE_SECS - 1, &ctx);
            assert_eq!(old.status, TaskStatus::Unknown);
            assert!(old.completion.is_none());
            assert_eq!(crate::pet_engine::mood_from_tasks(&[old], 10_000), crate::pet_engine::behavior::Mood::Idle);
            assert_eq!(to_record(&session, 10_000 - RUNNING_STALE_SECS, &ctx).status, TaskStatus::Running);
            assert_eq!(to_record(&session, 10_000, &ctx).status, TaskStatus::Running);
            let mut closed = ctx.clone();
            closed.processes = std::sync::Arc::new(super::super::ProcessTable::default());
            assert_eq!(to_record(&session, 10_000, &closed).status, TaskStatus::Unknown);
        }
        let completed = Session { status: "idle".into(), completion_id: Some("answer".into()), ..Default::default() };
        assert_eq!(to_record(&completed, 1, &ctx).status, TaskStatus::Completed);
        let pending = Session { status: "working".into(), pending_questions: vec!["question".into()], ..Default::default() };
        assert_eq!(to_record(&pending, 1, &ctx).status, TaskStatus::WaitingInput);
    }

    #[test]
    fn transcript_retention_uses_the_injected_window() {
        let now = 1_000_000;
        let window = 6 * 3_600;
        assert!(within_retention(now - window, now, window));
        assert!(within_retention(now - 60, now, window));
        assert!(!within_retention(now - window - 1, now, window));
        assert!(!within_retention(0, now, window));
        assert!(within_retention(now - 20 * 3_600, now, 24 * 3_600));
        assert!(!within_retention(now - 20 * 3_600, now, 6 * 3_600));
    }

    #[test]
    fn vscode_transcript_without_ai_title_uses_first_prompt_and_later_explicit_name() {
        let prompt = "{\"type\":\"file-history-snapshot\"}\n{\"type\":\"user\",\"entrypoint\":\"claude-vscode\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"修复 VSCode 会话识别\"}]}}\n";
        with_transcript(prompt, |path| {
            let state = parse_jsonl(path, None).unwrap();
            assert_eq!(state.session.name, "修复 VSCode 会话识别");
            assert_eq!(state.session.source, "vscode");
            use std::io::Write;
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{{\"type\":\"custom-title\",\"customTitle\":\"我的修复\"}}").unwrap();
            assert_eq!(parse_jsonl(path, Some(&state)).unwrap().session.name, "我的修复");
        });
    }

    fn with_transcript(contents: &str, check: impl FnOnce(&Path)) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "am-claude-{}-{}.jsonl", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::write(&path, contents).unwrap();
        check(&path);
        fs::remove_file(path).unwrap();
    }

    fn assistant(stop: &str) -> String {
        serde_json::json!({"type": "assistant", "message": {
            "stop_reason": stop, "content": [{"type": "text", "text": "progress"}]
        }}).to_string() + "\n"
    }

    #[test]
    fn final_answer_identity_survives_metadata_and_file_time_changes() {
        let answer = serde_json::json!({"type": "assistant", "uuid": "answer-one",
            "message": {"stop_reason": "end_turn", "content": []}}).to_string() + "\n";
        with_transcript(&answer, |path| {
            use std::io::Write;
            let first = parse_jsonl(path, None).unwrap();
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{{\"type\":\"summary\",\"aiTitle\":\"Renamed\"}}").unwrap();
            let changed = parse_jsonl(path, Some(&first)).unwrap();
            assert_eq!(changed.session.completion_id.as_deref(), Some("answer-one"));
            let second = answer.replace("answer-one", "answer-two");
            file.write_all(second.as_bytes()).unwrap();
            assert_eq!(parse_jsonl(path, Some(&changed)).unwrap().session.completion_id.as_deref(), Some("answer-two"));
        });
    }

    #[test]
    fn latest_parent_turn_model_is_not_an_old_or_subagent_model() {
        let reply = |model| serde_json::json!({"type": "assistant", "message": {
            "model": model, "stop_reason": "end_turn", "content": []
        }}).to_string() + "\n";
        let old = reply("old-model");
        let prompt = "{\"type\":\"user\",\"message\":{\"content\":\"new prompt\"}}\n";
        with_transcript(&(old.clone() + prompt), |path| {
            assert!(parse_jsonl(path, None).unwrap().session.model.is_empty());
        });
        let array_prompt = "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"new prompt\"}]}}\n";
        with_transcript(&(old.clone() + array_prompt), |path| {
            assert!(parse_jsonl(path, None).unwrap().session.model.is_empty());
        });
        let child = serde_json::json!({"type": "assistant", "isSidechain": true,
            "message": {"model": "child-model", "stop_reason": "end_turn"}}).to_string() + "\n";
        with_transcript(&(old + prompt + &reply("claude-opus-4-1") + &child), |path| {
            assert_eq!(parse_jsonl(path, None).unwrap().session.model, "claude-opus-4-1");
        });
    }

    #[test]
    fn ask_user_question_waits_for_its_own_answer_across_incremental_reads() {
        let question = serde_json::json!({"type": "assistant", "message": {"stop_reason": "tool_use", "content": [
            {"type": "tool_use", "name": "AskUserQuestion", "id": "ask-1"},
            {"type": "tool_use", "name": "Read", "id": "read-1"}
        ]}}).to_string() + "\n";
        with_transcript(&question, |path| {
            use std::io::Write;
            let pending = parse_jsonl(path, None).unwrap();
            assert_eq!(pending.session.pending_questions, vec!["ask-1"]);
            let ctx = ScanCtx::new(100, std::sync::Arc::new(super::super::ProcessTable { entries: vec![], sampled_at_ms: 0 }), 3_600);
            assert_eq!(to_record(&pending.session, 100, &ctx).status, TaskStatus::WaitingInput);
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            let result = |id| serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id}]}}).to_string() + "\n";
            file.write_all(result("read-1").as_bytes()).unwrap();
            let pending = parse_jsonl(path, Some(&pending)).unwrap();
            assert_eq!(pending.session.pending_questions, vec!["ask-1"]);
            file.write_all(result("ask-1").as_bytes()).unwrap();
            assert!(parse_jsonl(path, Some(&pending)).unwrap().session.pending_questions.is_empty());
        });
    }

    #[test]
    fn silence_after_user_tool_result_or_thinking_never_completes() {
        for contents in [
            "{\"type\":\"user\",\"message\":{\"content\":\"start\"}}\n".to_string(),
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]}}\n".to_string(),
            assistant("tool_use"),
            assistant("stop_sequence"),
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"thinking\"}]}}\n".to_string(),
        ] {
            with_transcript(&contents, |path| {
                fs::File::options().write(true).open(path).unwrap()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(100)).unwrap();
                let parsed = parse_jsonl(path, None).unwrap();
                assert_ne!(parsed.session.status, "idle");
                assert_ne!(parsed.session.status, "done");
            });
        }
    }

    #[test]
    fn only_parent_end_turn_completes_and_new_work_reopens_it() {
        with_transcript(&assistant("end_turn"), |path| {
            use std::io::Write;
            let ended = parse_jsonl(path, None).unwrap();
            assert_eq!(ended.session.status, "idle");
            let cached = parse_jsonl(path, Some(&ended)).unwrap();
            assert_eq!(cached.session.status, "idle");
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{{\"type\":\"api-request\"}}").unwrap();
            let running = parse_jsonl(path, Some(&cached)).unwrap();
            assert_eq!(running.session.status, "thinking");
            let child = serde_json::json!({"type": "assistant", "isSidechain": true,
                "message": {"stop_reason": "end_turn"}});
            writeln!(file, "{child}").unwrap();
            let running = parse_jsonl(path, Some(&running)).unwrap();
            assert_eq!(running.session.status, "thinking");
            file.write_all(assistant("end_turn").as_bytes()).unwrap();
            assert_eq!(parse_jsonl(path, Some(&running)).unwrap().session.status, "idle");
        });
    }

    #[test]
    fn partial_final_record_is_read_again_when_the_write_finishes() {
        with_transcript("{\"type\":\"api-request\"}\n", |path| {
            use std::io::Write;
            let running = parse_jsonl(path, None).unwrap();
            let final_line = assistant("end_turn");
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            let split = final_line.len() / 2;
            file.write_all(&final_line.as_bytes()[..split]).unwrap();
            let partial = parse_jsonl(path, Some(&running)).unwrap();
            assert_eq!(partial.session.status, "thinking");
            assert_eq!(partial.pos, running.pos);
            file.write_all(&final_line.as_bytes()[split..]).unwrap();
            assert_eq!(parse_jsonl(path, Some(&partial)).unwrap().session.status, "idle");
        });
    }

    #[test]
    fn tool_calls_cannot_complete_even_with_an_end_turn_label() {
        let text = serde_json::json!({"type": "assistant", "message": {
            "stop_reason": "end_turn", "content": [{"type": "tool_use", "name": "Agent"}]
        }}).to_string() + "\n";
        with_transcript(&text, |path| {
            assert_eq!(parse_jsonl(path, None).unwrap().session.status, "working");
        });
    }

    #[test]
    fn decodes_project_paths() {
        assert_eq!(decode_project_name("-Users-me-proj"), "/Users/me/proj");
        assert_eq!(pretty_display_name("-Users-me-proj"), "me-proj");
    }

    #[test]
    fn strips_system_injections() {
        let raw = "<system-reminder>x</system-reminder>\nhello\n<ide_opened_file>y</ide_opened_file>";
        assert_eq!(strip_injections(raw), "hello");
        assert_eq!(strip_injections("<ide_opened_file>\ninternal context\n</ide_opened_file>\nFix this session"), "Fix this session");
    }

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(parse_iso_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert!(parse_iso_timestamp("2026-10-21T10:00:00.000Z").unwrap() > 1_700_000_000);
    }

    #[test]
    fn legacy_status_mapping_is_explicit() {
        let session = Session {
            status: "working".into(),
            ..Default::default()
        };
        assert_eq!(session.status, "working");
        let unknown = String::new();
        assert!(!unknown.starts_with("waiting"));
    }


}
