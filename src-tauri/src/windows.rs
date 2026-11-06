//! Windows process sampling and verified application-level activation.
//! No Accessibility permission, shell command interpolation or title-based
//! conversation acknowledgement is used on this platform.

use crate::agents::ProcInfo;
use crate::model::*;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, POINT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

pub fn processes() -> Vec<ProcInfo> {
    let mut out = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE { return out; }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut valid = Process32FirstW(snapshot, &mut entry);
        while valid != 0 {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            out.push(ProcInfo { pid: entry.th32ProcessID, command: String::from_utf16_lossy(&entry.szExeFile[..len]) });
            valid = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    out
}

pub fn cursor_position() -> Option<(f64, f64)> {
    let mut point = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x as f64, point.y as f64))
}

pub fn system_is_chinese() -> bool {
    let mut name = [0u16; 85];
    let len = unsafe { windows_sys::Win32::Globalization::GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    len > 1 && String::from_utf16_lossy(&name[..len as usize - 1]).to_ascii_lowercase().starts_with("zh")
}

fn executable_matches(command: &str, names: &[&str]) -> bool {
    names.iter().any(|name| command.eq_ignore_ascii_case(name))
}

fn host_executables(task: &TaskRecord) -> &'static [&'static str] {
    if matches!(task.host.as_str(), "terminal" | "cli") {
        return &["WindowsTerminal.exe", "OpenConsole.exe", "pwsh.exe", "powershell.exe", "cmd.exe", "mintty.exe"];
    }
    if task.host == "vscode" { return &["Code.exe", "Code - Insiders.exe"]; }
    match task.agent_key.as_str() {
        "codex" => &["Codex.exe", "ChatGPT.exe"],
        "claude-code" => &["Code.exe", "Code - Insiders.exe"],
        "dsh" => &["DeepSeek Harness.exe", "dsh.exe"],
        "zcode" => &["ZCode.exe"],
        "antigravity" => &["Antigravity.exe"],
        _ => &[],
    }
}

struct WindowSearch { pids: Vec<u32>, windows: Vec<HWND> }

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> i32 {
    let search = &mut *(data as *mut WindowSearch);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if search.pids.contains(&pid) && IsWindowVisible(hwnd) != 0 {
        search.windows.push(hwnd);
    }
    1
}

fn activate_existing(task: &TaskRecord) -> bool {
    let names = host_executables(task);
    let mut search = WindowSearch {
        pids: processes().into_iter().filter(|p| executable_matches(&p.command, names)).map(|p| p.pid).collect(),
        windows: Vec::new(),
    };
    unsafe { EnumWindows(Some(collect_window), &mut search as *mut WindowSearch as LPARAM); }
    for hwnd in search.windows {
        unsafe {
            if IsIconic(hwnd) != 0 { ShowWindow(hwnd, SW_RESTORE); }
            SetForegroundWindow(hwnd);
        }
        // Windows may refuse foreground activation; a requested activation is
        // not evidence of success. Never acknowledge a conversation from this.
        for _ in 0..10 {
            if unsafe { GetForegroundWindow() } == hwnd { return true; }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
    false
}

fn vscode_executable() -> Option<PathBuf> {
    let candidates = [
        ("LOCALAPPDATA", "Programs/Microsoft VS Code/Code.exe"),
        ("ProgramFiles", "Microsoft VS Code/Code.exe"),
        ("ProgramFiles(x86)", "Microsoft VS Code/Code.exe"),
    ];
    candidates.into_iter().filter_map(|(key, suffix)| std::env::var_os(key).map(|root| PathBuf::from(root).join(suffix)))
        .find(|path| path.is_file())
}

pub fn jump(task: &TaskRecord) -> JumpOutcome {
    if task.target_validity == TargetValidity::Invalid {
        let mut outcome = JumpOutcome::failed(&task.id, task.level, task.invalid_reason.as_deref().unwrap_or("target_invalid"));
        outcome.next_step = Some("dismiss".into());
        return outcome;
    }
    if task.level == JumpLevel::L0 || (task.target.app_name.is_none()
        && task.target.bundle_id.is_none() && task.target.terminal_host.is_none()) {
        return JumpOutcome::failed(&task.id, task.level, "no_target");
    }
    // A source-confirmed VSCode session can request its workspace and URI. The
    // first Windows release verifies app focus only, so it never claims L3.
    if task.host == "vscode" {
        if let Some(code) = vscode_executable() {
            let mut command = std::process::Command::new(code);
            command.arg("--reuse-window");
            if let Some(path) = &task.target.workspace_path { command.arg(path); }
            let _ = command.spawn();
        }
        if let Some(uri) = &task.target.deep_link {
            if uri.starts_with("vscode://") {
                let _ = tauri_plugin_opener::open_url(uri, None::<&str>);
            }
        }
    }
    if activate_existing(task) {
        return JumpOutcome {
            task_id: task.id.clone(), kind: JumpOutcomeKind::Success,
            requested: task.level, achieved: JumpLevel::L1,
            reason_key: "opened_window".into(), detail: None,
            next_step: None, permission_needed: None,
        };
    }
    JumpOutcome::failed(&task.id, task.level, "windows_focus_failed")
}

pub fn diagnostics() -> crate::jump::JumpDiagnostics {
    use crate::jump::{AgentDiagnostics, DiagnosticStatus, JumpDiagnostics};
    let entries = processes();
    let status = || DiagnosticStatus { granted: true, detail: Some("not_required_on_windows".into()) };
    let agents = [
        ("codex", "Codex", "Codex.exe"), ("claude-code", "Claude Code / VSCode", "Code.exe"),
        ("dsh", "DSH", "DeepSeek Harness.exe"), ("zcode", "ZCode", "ZCode.exe"),
        ("antigravity", "Antigravity", "Antigravity.exe"),
    ].into_iter().map(|(key, name, exe)| {
        let running = entries.iter().any(|p| p.command.eq_ignore_ascii_case(exe));
        AgentDiagnostics { key, name, bundle_id: "", app_name: name,
            max_level: JumpLevel::L1, notes: "Windows: application focus only; exact conversation selection is not verified.",
            installed: running, running, deep_link: false }
    }).collect();
    JumpDiagnostics { accessibility: status(), automation: status(), window_access: status(),
        system_events: false, app_bundle_path: std::env::current_exe().ok().map(|p| p.display().to_string()), agents }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_process_snapshot_contains_this_process() {
        assert!(processes().iter().any(|p| p.pid == std::process::id()));
    }
    #[test]
    fn executable_matching_is_case_insensitive_and_exact() {
        assert!(executable_matches("CODE.EXE", &["Code.exe"]));
        assert!(!executable_matches("other-Code.exe", &["Code.exe"]));
    }
}
