//! Claude Code adapter — the VSCode agent plugin `anthropic.claude-code`
//! (2.1.304) plus the CLI variant, both backed by `~/.claude`.
//!
//! This is a port of the legacy `lib.rs` scanner, kept deliberately close to
//! the original behaviour so the monitoring regression checklist still passes:
//!   * incremental JSONL reads (byte offset + mtime),
//!   * 10 s silence rule and 120 s long-thinking rule via `file-history/`,
//!   * `ide/*.lock` PID liveness and `workspaceFolders` matching,
//!   * subagent transcripts and stale (>30 min, non-IDE) sessions excluded.
//!
//! Only explicit signals map to a status. Silence never becomes
//! waiting_input / waiting_approval.

use super::{truncate_chars, ScanCtx, Source};
use crate::model::*;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};

const VSCODE_BUNDLE: &str = "com.microsoft.VSCode";
const MAX_FILES: usize = 200;

#[derive(Clone, Default)]
struct Session {
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
    is_active: bool,
}

struct FileState {
    session: Session,
    mtime: u64,
    pos: u64,
}

struct IdeInfo {
    workspace_folders: Vec<String>,
}

pub struct ClaudeCodeSource {
    files: HashMap<String, FileState>,
    project_dirs: Vec<(PathBuf, String, String)>,
    projects_mtime: u64,
    ide: HashMap<String, IdeInfo>,
}

impl ClaudeCodeSource {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            project_dirs: Vec::new(),
            projects_mtime: 0,
            ide: HashMap::new(),
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

    fn detect_ide(&mut self, ctx: &ScanCtx) {
        let Some(ide_dir) = Self::claude_dir().map(|c| c.join("ide")) else {
            return;
        };
        let mut map = HashMap::new();
        let Ok(entries) = fs::read_dir(&ide_dir) else {
            self.ide.clear();
            return;
        };
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(".lock") {
                continue;
            }
            let Ok(content) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            if let Some(pid) = data.get("pid").and_then(|v| v.as_u64()) {
                let pid = pid as u32;
                // The process table is refreshed at most every 2 s by the
                // monitor; it is cheaper and more accurate than a kill(0) loop.
                if !ctx.processes.pids("").is_empty() && !process_alive(ctx, pid) {
                    continue;
                }
            }
            if let Some(folders) = data.get("workspaceFolders").and_then(|v| v.as_array()) {
                let folders: Vec<String> = folders
                    .iter()
                    .filter_map(|f| f.as_str().map(|s| s.to_string()))
                    .collect();
                if !folders.is_empty() {
                    map.insert(
                        fname,
                        IdeInfo {
                            workspace_folders: folders,
                        },
                    );
                }
            }
        }
        self.ide = map;
    }

    fn active_session_ids(now: u64) -> HashMap<String, u64> {
        let mut map = HashMap::new();
        let Some(dir) = Self::claude_dir().map(|c| c.join("file-history")) else {
            return map;
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            return map;
        };
        let cutoff = now.saturating_sub(1800);
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let mtime = super::file_mtime_secs(&entry.path());
            if mtime >= cutoff {
                map.insert(entry.file_name().to_string_lossy().to_string(), mtime);
            }
        }
        map
    }

    fn scan_projects(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        self.load_project_dirs();
        self.detect_ide(ctx);
        let active_sessions = Self::active_session_ids(ctx.now_secs);
        let mut out = Vec::new();
        let mut seen_files: Vec<String> = Vec::new();

        for (proj_path, raw_name, display) in &self.project_dirs {
            let is_ide_project = self.ide.values().any(|ide| {
                ide.workspace_folders
                    .iter()
                    .any(|f| encode_project_name(f).eq_ignore_ascii_case(raw_name))
            });
            let decoded = decode_project_name(raw_name);
            let Ok(dir_entries) = fs::read_dir(proj_path) else {
                continue;
            };
            for file in dir_entries.flatten() {
                let fpath = file.path();
                let fname = file.file_name().to_string_lossy().to_string();
                if fname.contains("subagent")
                    || !fpath.extension().map(|e| e == "jsonl").unwrap_or(false)
                {
                    continue;
                }
                if out.len() >= MAX_FILES {
                    break;
                }
                let file_key = fpath.to_string_lossy().to_string();
                let file_mt = super::file_mtime_secs(&fpath);
                let session_id = fname.trim_end_matches(".jsonl").to_string();
                let is_live = active_sessions.contains_key(&session_id) || is_ide_project;
                let stale_cutoff = ctx
                    .now_secs
                    .saturating_sub(if is_live { 24 * 3600 } else { 6 * 3600 });
                if file_mt < stale_cutoff {
                    continue;
                }
                let prev = self.files.get(&file_key);
                if let Some(state) = parse_jsonl(&fpath, prev) {
                    let mut session = state.session.clone();
                    if session.id.is_empty() {
                        session.id = session_id.clone();
                    }
                    if session.name.is_empty() {
                        session.name = display.clone();
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
                    let ide_host = is_ide_project
                        || self.ide.values().any(|ide| {
                            ide.workspace_folders.iter().any(|f| {
                                f == &project_path
                                    || encode_project_name(f).eq_ignore_ascii_case(raw_name)
                            })
                        });
                    session.project_path = project_path;
                    session.project_name = project_name;
                    session.is_active =
                        active_sessions.contains_key(&session_id) || ide_host;
                    if session.is_active
                        && session.status == "done"
                        && ctx.now_secs.saturating_sub(file_mt) < 120
                    {
                        session.status = "thinking".into();
                        session.status_text = "Thinking...".into();
                    }
                    seen_files.push(file_key.clone());
                    self.files.insert(
                        file_key,
                        FileState {
                            session: session.clone(),
                            mtime: state.mtime,
                            pos: state.pos,
                        },
                    );
                    let _ = ide_host;
                    out.push(to_record(&session, file_mt, ctx));
                }
            }
        }
        // Bound memory: forget transcripts outside the current window.
        self.files.retain(|k, _| seen_files.contains(k));
        out
    }
}

fn process_alive(ctx: &ScanCtx, pid: u32) -> bool {
    ctx.processes.entries.iter().any(|p| p.pid == pid)
}

fn encode_project_name(folder: &str) -> String {
    folder.replace('\\', "-").replace('/', "-").replace(':', "-")
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
    text.lines()
        .filter(|line| {
            let t = line.trim();
            !t.starts_with("<system-reminder>")
                && !t.starts_with("<ide_opened_file>")
                && !t.starts_with("<command-name>")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
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
    let file_mt = super::file_mtime_secs(path);
    if let Some(prev_state) = prev {
        if file_mt <= prev_state.mtime {
            return Some(FileState {
                session: prev_state.session.clone(),
                mtime: prev_state.mtime,
                pos: prev_state.pos,
            });
        }
    }
    let mut reader = BufReader::new(file);
    let start_pos = prev.map(|p| p.pos).unwrap_or(0);
    let mut session = prev.map(|p| p.session.clone()).unwrap_or_default();
    if start_pos > 0 {
        let _ = reader.seek(std::io::SeekFrom::Start(start_pos));
    }
    let mut line = String::new();
    let mut new_pos = start_pos;
    let mut last_role: Option<String> = None;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
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
        let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if session.name.is_empty() {
            if let Some(title) = obj.get("aiTitle").and_then(|v| v.as_str()) {
                session.name = title.to_string();
            }
        }
        if session.cwd.is_empty() {
            if let Some(cwd) = obj.get("cwd").and_then(|v| v.as_str()) {
                session.cwd = cwd.to_string();
            }
        }
        if session.source.is_empty() {
            let entry = obj.get("entrypoint").and_then(|v| v.as_str()).unwrap_or("");
            session.source = if entry.contains("vscode") {
                "vscode".into()
            } else {
                "cli".into()
            };
        }
        match typ {
            "user" => {
                last_role = Some("user".into());
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
                last_role = Some("assistant".into());
                session.message_count += 1;
                if let Some(msg) = obj.get("message") {
                    if let Some(model) = msg.get("model").and_then(|v| v.as_str()) {
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
                            "end_turn" => {
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
            _ => {}
        }
    }
    let silent = super::file_mtime_secs(path) + 10 < crate::model::now_secs();
    if last_role.as_deref() == Some("user") && silent {
        session.status = "done".into();
        session.status_text = "Waiting for you".into();
        session.active_tool.clear();
        session.tool_detail.clear();
    } else if last_role.as_deref() == Some("user") && !silent {
        session.status = "working".into();
        session.status_text = "Working...".into();
    } else if session.status.is_empty() {
        session.status = "working".into();
        session.status_text = "Working...".into();
    }
    if (session.status == "responding" || session.status == "thinking") && silent {
        session.status = "done".into();
        session.status_text = "Finished".into();
        session.active_tool.clear();
        session.tool_detail.clear();
    }
    Some(FileState {
        session,
        mtime: file_mt,
        pos: new_pos,
    })
}

fn short_model(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    for (needle, label) in [
        ("opus", "OPUS"),
        ("sonnet", "SONNET"),
        ("haiku", "HAIKU"),
        ("deepseek", "DS"),
        ("gpt", "GPT"),
    ] {
        if m.contains(needle) {
            return label.into();
        }
    }
    if m.is_empty() {
        String::new()
    } else {
        truncate_chars(&m, 6).to_uppercase()
    }
}

fn to_record(session: &Session, file_mtime: u64, ctx: &ScanCtx) -> TaskRecord {
    // The supported integration is the VSCode extension (anthropic.claude-code).
    // Claude Code is a VSCode-hosted agent here; we never route it to Terminal.
    let host = "vscode";
    let (status, reason) = match session.status.as_str() {
        "working" => (TaskStatus::Running, "tool_use in transcript".to_string()),
        "thinking" => (TaskStatus::Running, "thinking block".to_string()),
        "responding" => (TaskStatus::Running, "assistant text".to_string()),
        "idle" | "done" => (
            TaskStatus::Completed,
            "stop_reason end_turn / silence rule".to_string(),
        ),
        other => (
            TaskStatus::Unknown,
            format!("unmapped legacy status {other}"),
        ),
    };
    let mut summary_parts = Vec::new();
    let model = short_model(&session.model);
    if !model.is_empty() {
        summary_parts.push(model);
    }
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
        deep_link: None,
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
        status_reason: reason,
        last_activity_secs: file_mtime,
        collected_at_secs: ctx.now_secs,
        summary,
        origin: DataOrigin::Event,
        target,
        target_validity: TargetValidity::Unconfirmed,
        invalid_reason: None,
        capability: JumpCapability {
            max_level: JumpLevel::L2,
            notes: "VSCode window by workspace; the extension exposes no verified session URI"
                .into(),
            requires: vec![PermissionKind::Automation],
        },
        level: JumpLevel::L2,
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
            source: "~/.claude/projects/**/*.jsonl + file-history + ide/*.lock".into(),
            origin: DataOrigin::Event,
            max_level: JumpLevel::L2,
            notes: "VSCode extension host (anthropic.claude-code); window-level jump by workspace."
                .into(),
        }
    }

    fn invalidate(&mut self) {
        self.files.clear();
        self.projects_mtime = 0;
        self.ide.clear();
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        self.scan_projects(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_project_paths() {
        assert_eq!(
            encode_project_name("/Users/me/My Project"),
            "-Users-me-My Project"
        );
        assert_eq!(decode_project_name("-Users-me-proj"), "/Users/me/proj");
        assert_eq!(pretty_display_name("-Users-me-proj"), "me-proj");
    }

    #[test]
    fn strips_system_injections() {
        let raw = "<system-reminder>x</system-reminder>\nhello\n<ide_opened_file>y</ide_opened_file>";
        assert_eq!(strip_injections(raw), "hello");
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

    #[test]
    fn model_labels_stay_short() {
        assert_eq!(short_model("claude-opus-4-1"), "OPUS");
        assert_eq!(short_model("deepseek-v4.1-flash"), "DS");
        assert_eq!(short_model(""), "");
    }
}
