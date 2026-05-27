use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

// ── Data types ──────────────────────────────────────────────────

#[derive(Clone, Serialize, Debug, Default)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub status: String,
    pub status_text: String,
    pub file_path: String,
    pub project_path: String,
    pub project_name: String,
    pub model: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
    pub active_tool: String,
    pub tool_detail: String,
    pub uptime_secs: u64,
    pub message_count: u32,
    pub cwd: String,
    pub source: String,
    pub last_user_msg: String,
    pub is_active: bool,
}

#[derive(Clone, Serialize, Debug)]
pub struct Project {
    pub sessions: Vec<Session>,
}

pub type SessionMap = HashMap<String, Project>;

#[derive(Clone, Debug)]
struct FileState {
    session: Session,
    mtime: u64,
    pos: u64,
}

pub struct AppState {
    pub sessions: Mutex<SessionMap>,
    pub files: Mutex<HashMap<String, FileState>>,
}

// ── Paths ───────────────────────────────────────────────────────

fn claude_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}

fn projects_dir() -> Option<PathBuf> {
    claude_dir().map(|c| c.join("projects"))
}

fn file_history_dir() -> Option<PathBuf> {
    claude_dir().map(|c| c.join("file-history"))
}

fn ide_dir() -> Option<PathBuf> {
    claude_dir().map(|c| c.join("ide"))
}

// ── Encoding ────────────────────────────────────────────────────

fn encode_project_name(folder: &str) -> String {
    folder.replace('\\', "-").replace('/', "-").replace(':', "-")
}

fn decode_project_name(encoded: &str) -> String {
    if let Some(rest) = encoded.strip_prefix(|c: char| c.is_ascii_alphabetic()) {
        if let Some(rest) = rest.strip_prefix("--") {
            let drive = &encoded[..1];
	    #[cfg(target_os = "windows")]
	    { return format!("{}:\\{}", drive.to_uppercase(), rest.replace("--", "\\")); }
	    #[cfg(not(target_os = "windows"))]
	    { return format!("/{}", rest.replace("--", "/")); }
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
    // Strip drive prefix (e.g., "d--" → ""), keep dashes as separators
    let path = if let Some(rest) = raw.strip_prefix(|c: char| c.is_ascii_alphabetic()) {
        rest.strip_prefix("--").unwrap_or(rest)
    } else {
        raw
    };
    // Keep only last 2 dash-separated segments, joined with "-"
    let parts: Vec<&str> = path.split('-').filter(|s| !s.is_empty()).collect();
    if parts.len() > 2 {
        parts[parts.len() - 2..].join("-")
    } else {
        path.to_string()
    }
}

// ── Time helpers ────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn file_mtime(path: &std::path::Path) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
        .unwrap_or(0)
}

fn today_cutoff() -> u64 {
    // 30-min window for non-IDE sessions (matches claude-super-monitor)
    now_secs().saturating_sub(1800)
}

// ── IDE & process detection ─────────────────────────────────────

struct IdeInfo {
    workspace_folders: Vec<String>,
}

fn detect_active_ide_windows(pid_cache: &mut HashMap<u32, bool>) -> HashMap<String, IdeInfo> {
    let mut map = HashMap::new();
    let ide_path = match ide_dir() {
        Some(p) => p,
        None => return map,
    };

    let entries = match fs::read_dir(&ide_path) {
        Ok(e) => e,
        Err(_) => return map,
    };

    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        if !fname.ends_with(".lock") {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let data: serde_json::Value = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if let Some(pid) = data.get("pid").and_then(|v| v.as_u64()) {
            let pid32 = pid as u32;
            let alive = *pid_cache.entry(pid32).or_insert_with(|| is_process_alive(pid32));
            if !alive {
                continue;
            }
        }

        if let Some(folders) = data.get("workspaceFolders").and_then(|v| v.as_array()) {
            let folders_str: Vec<String> = folders
                .iter()
                .filter_map(|f| f.as_str().map(|s| s.to_string()))
                .collect();
            if !folders_str.is_empty() {
                map.insert(fname.clone(), IdeInfo {
                    workspace_folders: folders_str,
                });
            }
        }
    }

    map
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| {
                let out = String::from_utf8_lossy(&o.stdout);
                out.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}

fn get_active_session_ids() -> HashMap<String, u64> {
    let cutoff = today_cutoff();
    let fh_dir = match file_history_dir() {
        Some(p) => p,
        None => return HashMap::new(),
    };

    let mut map = HashMap::new();
    if let Ok(entries) = fs::read_dir(&fh_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let session_id = entry.file_name().to_string_lossy().to_string();
            let mt = file_mtime(&entry.path());
            if mt >= cutoff {
                map.insert(session_id, mt);
            }
        }
    }
    map
}

// ── Strip system injections ─────────────────────────────────────

fn strip_injections(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("<system-reminder>")
                && !trimmed.starts_with("<ide_opened_file>")
                && !trimmed.starts_with("<command-name>")
        })
        .collect();
    lines.join("\n").trim().to_string()
}

// ── Tool name mapper ────────────────────────────────────────────

fn tool_display_name(tool: &str) -> &str {
    match tool {
        "Bash" => "Running",
        "Write" => "Writing",
        "Edit" => "Editing",
        "MultiEdit" => "Editing",
        "Read" => "Reading",
        "Glob" => "Searching",
        "Grep" => "Searching",
        "TodoWrite" => "Planning",
        "Agent" => "Delegating",
        "WebFetch" => "Fetching",
        "WebSearch" => "Searching",
        "NotebookEdit" => "Editing",
        "AskUserQuestion" => "Asking",
        "EnterPlanMode" => "Planning",
        "ExitPlanMode" => "Planning",
        _ => tool,
    }
}

// ── JSONL parser ────────────────────────────────────────────────

fn parse_jsonl(path: &std::path::Path, prev: Option<&FileState>) -> Option<FileState> {
    let file = fs::File::open(path).ok()?;
    let file_mt = file_mtime(path);

    // If file hasn't changed, return cached state
    if let Some(prev_state) = prev {
        if file_mt <= prev_state.mtime {
            // Update uptime only
            let mut s = prev_state.session.clone();
            if s.uptime_secs > 0 {
                s.uptime_secs = s.uptime_secs.max(1);
            }
            return Some(FileState {
                session: s,
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

        let obj: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Timestamp
        if session.uptime_secs == 0 {
            if let Some(ts) = obj.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(start) = parse_iso_timestamp(ts) {
                    session.uptime_secs = now_secs().saturating_sub(start);
                }
            }
        }

        // Session name from aiTitle
        if session.name.is_empty() {
            if let Some(title) = obj.get("aiTitle").and_then(|v| v.as_str()) {
                session.name = title.to_string();
            }
        }

        // CWD
        if session.cwd.is_empty() {
            if let Some(cwd) = obj.get("cwd").and_then(|v| v.as_str()) {
                session.cwd = cwd.to_string();
            }
        }

        // Source
        if session.source.is_empty() {
            let entry = obj.get("entrypoint").and_then(|v| v.as_str()).unwrap_or("");
            session.source = if entry.contains("vscode") { "vscode".into() } else { "cli".into() };
        }

        match typ {
            "user" => {
                last_role = Some("user".into());
                session.message_count += 1;
                if let Some(msg) = obj.get("message") {
                    if let Some(content) = msg.get("content") {
                        if let Some(text) = content.as_str() {
                            let cleaned = strip_injections(text);
                            let preview: String = cleaned.chars().take(80).collect();
                            if !preview.is_empty() {
                                session.last_user_msg = preview;
                            }
                        }
                    }
                }
            }
            "assistant" => {
                last_role = Some("assistant".into());
                session.message_count += 1;

                if let Some(msg) = obj.get("message") {
                    // Model
                    if let Some(model) = msg.get("model").and_then(|v| v.as_str()) {
                        session.model = model.to_string();
                    }
                    // Tokens
                    if let Some(usage) = msg.get("usage") {
                        session.tokens_in = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(session.tokens_in);
                        session.tokens_out = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(session.tokens_out);
                    }
                    // Cost
                    if let Some(cost) = obj.get("costUSD").and_then(|v| v.as_f64()) {
                        session.cost = cost;
                    }
                    // Content analysis
                    if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
                        for item in content.iter().rev() {
                            let ctype = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match ctype {
                                "tool_use" => {
                                    if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                        session.active_tool = name.to_string();
                                        session.status_text = format!("{} {}", tool_display_name(name), name);
                                        session.status = "working".into();
                                        if let Some(input) = item.get("input") {
                                            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                                                session.tool_detail = cmd.chars().take(60).collect();
                                            } else if let Some(fp) = input.get("file_path").and_then(|v| v.as_str()) {
                                                session.tool_detail = short_name(fp);
                                            } else if let Some(pattern) = input.get("pattern").and_then(|v| v.as_str()) {
                                                session.tool_detail = pattern.chars().take(40).collect();
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
                    // Stop reason
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

    // Final status
    // Only mark as "done" if the file has been silent for 10+ seconds.
    // This prevents agent sub-steps from flickering between done/working.
    let silent_secs = now_secs().saturating_sub(file_mt);
    let silent = silent_secs > 10;

    if last_role.as_deref() == Some("user") && silent {
        // User sent the last message and file hasn't changed → truly waiting
        session.status = "done".into();
        session.status_text = "Waiting for you".into();
        session.active_tool.clear();
        session.tool_detail.clear();
    } else if last_role.as_deref() == Some("user") && !silent {
        // User just sent input (or tool_result), assistant may still respond
        // Keep the session as "working" to indicate activity
        session.status = "working".into();
        session.status_text = "Working...".into();
    } else if session.status.is_empty() {
        session.status = "working".into();
        session.status_text = "Working...".into();
    }

    // Silence detection for stale responding/thinking sessions
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

fn parse_iso_timestamp(ts: &str) -> Result<u64, ()> {
    let ts = ts.trim_end_matches('Z');
    let parts: Vec<&str> = ts.split('T').collect();
    if parts.len() != 2 { return Err(()); }
    let date: Vec<&str> = parts[0].split('-').collect();
    let time: Vec<&str> = parts[1].split(':').collect();
    if date.len() != 3 || time.len() < 3 { return Err(()); }

    let year: u64 = date[0].parse().map_err(|_| ())?;
    let month: u64 = date[1].parse().map_err(|_| ())?;
    let day: u64 = date[2].parse().map_err(|_| ())?;
    let hour: u64 = time[0].parse().map_err(|_| ())?;
    let min: u64 = time[1].parse().map_err(|_| ())?;
    let sec: u64 = time[2].parse::<f64>().map_err(|_| ())? as u64;

    let mut days = (year - 1970) * 365 + (year - 1968) / 4;
    for m in 1..month {
        days += match m {
            1|3|5|7|8|10|12 => 31, 4|6|9|11 => 30, 2 => 29, _ => 0,
        };
    }
    days += day - 1;
    Ok(days * 86400 + hour * 3600 + min * 60 + sec)
}

// ── Scanner (mtime-aware incremental) ───────────────────────────

fn scan_all_sessions(
    files: &mut HashMap<String, FileState>,
    pid_cache: &mut HashMap<u32, bool>,
) -> SessionMap {
    let mut map: SessionMap = HashMap::new();
    let projects_path = match projects_dir() {
        Some(p) => p,
        None => return map,
    };

    let active_sessions = get_active_session_ids();
    let ide_windows = detect_active_ide_windows(pid_cache);
    let cutoff = today_cutoff();

    let entries = match fs::read_dir(&projects_path) {
        Ok(e) => e,
        Err(_) => return map,
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let proj_path = entry.path();
        let raw_name = entry.file_name().to_string_lossy().to_string();
        let decoded = decode_project_name(&raw_name);
        let display = if raw_name.is_empty() {
            short_name(&decoded)
        } else {
            pretty_display_name(&raw_name)
        };
        let mut sessions = Vec::new();

        // Check if project is in active IDE
        let is_ide_project = ide_windows.values().any(|ide| {
            ide.workspace_folders
                .iter()
                .any(|f| encode_project_name(f).eq_ignore_ascii_case(&raw_name))
        });

        if let Ok(dir_entries) = fs::read_dir(&proj_path) {
            for file in dir_entries.flatten() {
                let fpath = file.path();
                let fname = file.file_name().to_string_lossy().to_string();

                if fname.contains("subagent") {
                    continue;
                }
                if !fpath.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    continue;
                }

                let file_key = fpath.to_string_lossy().to_string();
                let session_id = fname.trim_end_matches(".jsonl").to_string();
                let file_mt = file_mtime(&fpath);

                // Skip stale files (not today and not in active IDE)
                if file_mt < cutoff && !is_ide_project {
                    continue;
                }

                // Parse (uses mtime-aware cache)
                let prev = files.get(&file_key);
                let file_state = parse_jsonl(&fpath, prev);

                if let Some(mut state) = file_state {
                    // Fill in metadata
                    if state.session.id.is_empty() {
                        state.session.id = session_id.clone();
                    }
                    if state.session.name.is_empty() {
                        state.session.name = session_id[..8.min(session_id.len())].to_string();
                    }
                    state.session.file_path = file_key.clone();
                    state.session.project_path = decoded.clone();
                    state.session.project_name = display.clone();
                    state.session.is_active = active_sessions.contains_key(&session_id) || is_ide_project;

                    // For sessions that are active (file-history mtime is recent),
                    // don't show as "done" even if file hasn't changed recently.
                    // Long-thinking sessions have stale JSONL but active file-history.
                    if state.session.is_active && state.session.status == "done" {
                        let silent_secs = now_secs().saturating_sub(file_mt);
                        if silent_secs < 120 {
                            // Still active per file-history, and <2min since last write
                            state.session.status = "thinking".into();
                            state.session.status_text = "Thinking...".into();
                        }
                    }

                    sessions.push(state.session.clone());
                    files.insert(file_key, state);
                }
            }
        }

        if !sessions.is_empty() {
            map.insert(display, Project { sessions });
        }
    }

    map
}

fn build_session_map(
    files: &mut HashMap<String, FileState>,
    pid_cache: &mut HashMap<u32, bool>,
) -> SessionMap {
    scan_all_sessions(files, pid_cache)
}

// ── Tauri commands ──────────────────────────────────────────────

#[tauri::command]
fn ping() -> String { "pong".into() }

#[tauri::command]
fn get_sessions_json(state: tauri::State<'_, AppState>) -> String {
    let sessions = state.sessions.lock().unwrap();
    serde_json::to_string(&*sessions).unwrap_or_else(|_| "{}".into())
}

#[tauri::command]
fn refresh(state: tauri::State<'_, AppState>) -> String {
    let mut files = state.files.lock().unwrap();
    let data = {
        let mut pid_cache = HashMap::new();
        build_session_map(&mut files, &mut pid_cache)
    };
    let mut sessions = state.sessions.lock().unwrap();
    *sessions = data.clone();
    drop(sessions);
    serde_json::to_string(&data).unwrap_or_else(|_| "{}".into())
}

#[tauri::command]
fn set_clickthrough(enabled: bool, app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_ignore_cursor_events(enabled);
    }
}

#[tauri::command]
fn set_size(scale: String, app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let h: f64 = match scale.as_str() {
            "m" => 350.0,   // compact
            "l" => 700.0,   // expanded
            _ => 520.0,      // default
        };
        // Only change height, keep width at 360
        let _ = win.set_size(tauri::PhysicalSize::new(360.0, h));
    }
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

#[tauri::command]
fn quit_app() {
    std::process::exit(0);
}

#[tauri::command]
fn open_in_vscode(file_path: String, project_path: String) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = std::process::Command::new("code")
            .args(["--goto", &file_path, &project_path])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .or_else(|_| {
                std::process::Command::new("code")
                    .arg(&project_path)
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
            });
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("code")
            .args(["--goto", &file_path, &project_path])
            .spawn()
            .or_else(|_| std::process::Command::new("code").arg(&project_path).spawn());
    }
}

// ── Watcher ─────────────────────────────────────────────────────

fn start_watcher(app: AppHandle) {
    use notify::{EventKind, RecursiveMode, Watcher};
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();

    // Use PollWatcher for reliable cross-platform detection
    let config = notify::Config::default().with_poll_interval(std::time::Duration::from_secs(2));
    let mut watcher = match notify::PollWatcher::new(tx, config) {
        Ok(w) => w,
        Err(_) => return,
    };

    if let Some(p) = projects_dir() {
        if p.exists() {
            let _ = watcher.watch(&p, RecursiveMode::Recursive);
        }
    }
    if let Some(p) = file_history_dir() {
        if p.exists() {
            let _ = watcher.watch(&p, RecursiveMode::Recursive);
        }
    }
    if let Some(p) = ide_dir() {
        if p.exists() {
            let _ = watcher.watch(&p, RecursiveMode::Recursive);
        }
    }

    std::thread::spawn(move || {
        while let Ok(Ok(event)) = rx.recv() {
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                    // Debounce: wait for quiet period
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    // Drain any pending events
                    while let Ok(Ok(_)) = rx.try_recv() {}
                    // Refresh
                    if let Some(state) = app.try_state::<AppState>() {
                        let mut files = state.files.lock().unwrap();
                        let data = {
                            let mut pid_cache = HashMap::new();
                            build_session_map(&mut files, &mut pid_cache)
                        };
                        let json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
                        let mut sessions = state.sessions.lock().unwrap();
                        *sessions = data;
                        drop(sessions);
                        let _ = app.emit("sessions-updated", &json);
                    }
                }
                _ => {}
            }
        }
    });
}

// ── Window helpers ──────────────────────────────────────────────

fn show_window(win: &tauri::WebviewWindow) {
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}

fn toggle_visible(win: &tauri::WebviewWindow) {
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        show_window(win);
    }
}

// ── Tray ────────────────────────────────────────────────────────

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::TrayIconEvent;

    let show_item = MenuItemBuilder::with_id("show", "Show").build(app)?;
    let refresh_item = MenuItemBuilder::with_id("refresh", "Refresh").build(app)?;
    let prefs_item = MenuItemBuilder::with_id("prefs", "Preferences").build(app)?;
    let ct_item = MenuItemBuilder::with_id("toggle-ct", "Toggle Click-Thru").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&refresh_item)
        .item(&prefs_item)
        .item(&ct_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let tray = app.tray_by_id("main-tray").unwrap();
    tray.set_menu(Some(menu))?;

    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click { button, .. } = event {
            if button == tauri::tray::MouseButton::Left {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    toggle_visible(&win);
                }
            }
        }
    });

    tray.on_menu_event(move |app, event| {
        let id = event.id().as_ref();
        match id {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    show_window(&win);
                }
            }
            "refresh" => { let _ = app.emit("tray-action", "refresh"); }
            "prefs" => {
                if let Some(win) = app.get_webview_window("main") { show_window(&win); }
                let _ = app.emit("tray-action", "prefs");
            }
            "toggle-ct" => { let _ = app.emit("tray-action", "toggle-ct"); }
            "quit" => std::process::exit(0),
            _ => {}
        }
    });

    Ok(())
}

// ── Entry ───────────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .manage(AppState {
            sessions: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            get_sessions_json,
            refresh,
            set_clickthrough,
            set_size,
            hide_window,
            quit_app,
            open_in_vscode,
        ])
        .setup(|app| {
            let state = app.state::<AppState>();
            let mut files = state.files.lock().unwrap();
            let data = {
                let mut pid_cache = HashMap::new();
                build_session_map(&mut files, &mut pid_cache)
            };
            let mut sessions = state.sessions.lock().unwrap();
            *sessions = data;
            drop(sessions);
            drop(files);
            setup_tray(app.handle())?;
            start_watcher(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
