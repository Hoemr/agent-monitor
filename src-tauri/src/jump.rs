//! Graded jump executor.
//!
//! L3 session → L2 window → L1 app → L0 nothing, with real verification:
//! a level is only reported as achieved when a read-back signal confirms it.
//! Degradation happens only on a known capability limit; a session id is never
//! reused to guess another session.
//!
//! All OS interaction is through `open` and `osascript` (System Events), which
//! is what Apple exposes without private APIs. No retry loops, no polling.

use crate::model::*;
use std::process::Command;
use std::time::Duration;

pub const ACCESSIBILITY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const AUTOMATION_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation";

pub fn open_permission_settings(kind: PermissionKind) {
    let pane = match kind {
        PermissionKind::Accessibility => ACCESSIBILITY_PANE,
        PermissionKind::Automation => AUTOMATION_PANE,
    };
    let _ = Command::new("open").arg(pane).status();
}

// ── Public entry ────────────────────────────────────────────────

pub fn jump(task: &TaskRecord) -> JumpOutcome {
    let requested = task.level;
    if task.target_validity == TargetValidity::Invalid {
        return JumpOutcome {
            task_id: task.id.clone(),
            kind: JumpOutcomeKind::Failed,
            requested,
            achieved: JumpLevel::L0,
            reason_key: task
                .invalid_reason
                .clone()
                .unwrap_or_else(|| "target_invalid".into()),
            detail: None,
            next_step: Some("dismiss".into()),
            permission_needed: None,
        };
    }
    if requested <= JumpLevel::L0 {
        return JumpOutcome::failed(&task.id, requested, "no_target");
    }

    let max = task.capability.max_level;
    let mut first_error = None;

    // L3 — session / tab level.
    if max >= JumpLevel::L3 && task.target.deep_link.is_some() {
        match try_session_level(task) {
            Attempt::Done(outcome) => return outcome,
            Attempt::NotPossible(reason) => first_error = Some(reason),
        }
    }
    // L2 — specific window.
    if max >= JumpLevel::L2 {
        match try_window_level(task) {
            Attempt::Done(outcome) => return outcome,
            Attempt::NotPossible(reason) => {
                first_error.get_or_insert(reason);
            }
        }
    }
    // L1 — application activation.
    match try_app_level(task) {
        Attempt::Done(mut outcome) => {
            // App-level activation is always a degradation for a session that
            // has an identity the user expected to reach.
            let needs_session = requested > JumpLevel::L1 || task.session_id.is_some();
            if outcome.reason_key == "activated_app" && needs_session {
                outcome.kind = JumpOutcomeKind::Degraded;
                outcome.reason_key = first_error
                    .clone()
                    .unwrap_or_else(|| "activated_app_only".into());
            }
            outcome
        }
        Attempt::NotPossible(reason) => {
            let needs = if reason == "needs_accessibility" {
                Some(PermissionKind::Accessibility)
            } else if reason == "needs_automation" {
                Some(PermissionKind::Automation)
            } else {
                None
            };
            let mut outcome =
                JumpOutcome::failed(&task.id, requested, first_error.as_deref().unwrap_or(&reason));
            outcome.permission_needed = needs;
            if needs.is_some() {
                outcome.next_step = Some("grant_permission".into());
            }
            outcome
        }
    }
}

enum Attempt {
    Done(JumpOutcome),
    NotPossible(String),
}

// ── Level implementations ───────────────────────────────────────

fn try_session_level(task: &TaskRecord) -> Attempt {
    // Terminal tab selection (Claude Code CLI and any terminal-hosted agent).
    if let Some(host) = task.target.terminal_host.as_deref() {
        if host == "Terminal" {
            return try_terminal_tab(task);
        }
    }
    // Verified deep link.
    let Some(link) = task.target.deep_link.as_deref() else {
        return Attempt::NotPossible("no_session_route".into());
    };
    let process = process_name(task);
    let expected_title = task.target.window_title.as_deref().unwrap_or("");
    // Some hosts (Codex) show an AI-generated window title that differs from
    // the stored thread title, so capture the visible titles before the jump
    // and accept a real navigation (title change) as session-level evidence.
    let before_titles = ax_window_titles(&process).unwrap_or_default();
    match open_url(link) {
        Ok(()) => {}
        Err(reason) => return Attempt::NotPossible(reason),
    }
    std::thread::sleep(Duration::from_millis(900));
    let front = frontmost_app();
    if front.as_deref() != Some(process.as_str()) {
        return Attempt::NotPossible("deep_link_unverified".into());
    }
    let after_titles = match ax_window_titles(&process) {
        Some(t) => t,
        None => return Attempt::NotPossible(ax_failure_reason().to_string()),
    };
    let mut navigated = after_titles
        .iter()
        .find(|t| !before_titles.contains(t))
        .cloned();
    if navigated.is_none() && !expected_title.is_empty() {
        navigated = best_title_match(&after_titles, expected_title);
    }
    if let Some(detail) = navigated {
        return Attempt::Done(JumpOutcome {
            task_id: task.id.clone(),
            kind: JumpOutcomeKind::Success,
            requested: task.level,
            achieved: JumpLevel::L3,
            reason_key: "opened_session".into(),
            detail: Some(detail),
            next_step: None,
            permission_needed: None,
        });
    }
    // The app came forward, but we cannot prove which session is visible.
    // Never claim L3 on "command sent": degrade to the window/app ladder.
    match try_window_level(task) {
        Attempt::Done(mut outcome) => {
            outcome.reason_key = "deep_link_unverified".into();
            outcome.kind = JumpOutcomeKind::Degraded;
            Attempt::Done(outcome)
        }
        Attempt::NotPossible(_) => Attempt::NotPossible("deep_link_unverified".into()),
    }
}

fn try_window_level(task: &TaskRecord) -> Attempt {
    let process = process_name(task);
    let Some(expected) = task.target.window_title.as_deref().filter(|s| !s.is_empty()) else {
        return Attempt::NotPossible("no_window_hint".into());
    };
    let Some(titles) = ax_window_titles(&process) else {
        return Attempt::NotPossible(ax_failure_reason().to_string());
    };
    let matches = matching_windows(&titles, expected);
    if matches.is_empty() {
        return Attempt::NotPossible("window_not_found".into());
    }
    let ambiguous = matches.len() > 1;
    let chosen = matches[0].clone();
    match raise_window(&process, &chosen) {
        Ok(()) => {}
        Err(reason) => return Attempt::NotPossible(reason),
    }
    std::thread::sleep(Duration::from_millis(350));
    let front = frontmost_app();
    let achieved = JumpLevel::L2;
    let verified = front.as_deref() == Some(process.as_str());
    if !verified {
        return Attempt::NotPossible("focus_unconfirmed".into());
    }
    Attempt::Done(JumpOutcome {
        task_id: task.id.clone(),
        kind: if ambiguous {
            JumpOutcomeKind::Degraded
        } else {
            JumpOutcomeKind::Success
        },
        requested: task.level,
        achieved,
        reason_key: if ambiguous {
            "window_ambiguous".into()
        } else {
            "opened_window".into()
        },
        detail: Some(chosen),
        next_step: None,
        permission_needed: None,
    })
}

fn try_app_level(task: &TaskRecord) -> Attempt {
    let Some(bundle) = task.target.bundle_id.as_deref() else {
        return Attempt::NotPossible("no_target".into());
    };
    let process = process_name(task);
    match Command::new("open").args(["-b", bundle]).status() {
        Ok(status) if status.success() => {}
        _ => {
            // Fall back to the application name before giving up.
            let app_name = task.target.app_name.clone().unwrap_or_default();
            if app_name.is_empty()
                || !Command::new("open")
                    .args(["-a", &app_name])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            {
                return Attempt::NotPossible("app_not_found".into());
            }
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    let front = frontmost_app();
    if front.as_deref() == Some(process.as_str()) {
        Attempt::Done(JumpOutcome {
            task_id: task.id.clone(),
            kind: JumpOutcomeKind::Success,
            requested: task.level,
            achieved: JumpLevel::L1,
            reason_key: "activated_app".into(),
            detail: Some(process),
            next_step: None,
            permission_needed: None,
        })
    } else {
        // Activation was requested successfully but could not be confirmed.
        Attempt::Done(JumpOutcome {
            task_id: task.id.clone(),
            kind: JumpOutcomeKind::Unconfirmed,
            requested: task.level,
            achieved: JumpLevel::L1,
            reason_key: "focus_unconfirmed".into(),
            detail: Some(process),
            next_step: None,
            permission_needed: None,
        })
    }
}

fn try_terminal_tab(task: &TaskRecord) -> Attempt {
    let host = task.target.terminal_host.as_deref().unwrap_or("Terminal");
    if host != "Terminal" {
        return Attempt::NotPossible("unsupported_terminal_host".into());
    }
    let tty = match task.target.terminal_tty.clone() {
        Some(t) if !t.is_empty() => t,
        _ => {
            let cwd = task.target.workspace_path.as_deref().unwrap_or("");
            match resolve_terminal_tty(agent_process_needle(&task.agent_key), cwd) {
                Some(t) => t,
                None => return Attempt::NotPossible("terminal_tab_not_found".into()),
            }
        }
    };
    let Some(selected) = select_terminal_tab(&tty) else {
        return Attempt::NotPossible(terminal_failure_reason().to_string());
    };
    match selected {
        Some(actual) if actual == tty => Attempt::Done(JumpOutcome {
            task_id: task.id.clone(),
            kind: JumpOutcomeKind::Success,
            requested: task.level,
            achieved: JumpLevel::L3,
            reason_key: "opened_terminal_tab".into(),
            detail: Some(tty),
            next_step: None,
            permission_needed: None,
        }),
        _ => Attempt::NotPossible("terminal_tab_not_found".into()),
    }
}

fn agent_process_needle(agent_key: &str) -> &'static str {
    match agent_key {
        "claude-code" => "claude",
        "codex" => "codex",
        "dsh" => "dsh",
        "zcode" => "zcode",
        _ => "",
    }
}

// ── AX helpers ──────────────────────────────────────────────────

fn process_name(task: &TaskRecord) -> String {
    if let Some(name) = task.target.app_name.as_deref() {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    match task.target.bundle_id.as_deref() {
        Some("com.openai.codex") => "ChatGPT".into(),
        Some("com.deepseek.dsh") => "DeepSeek Harness".into(),
        Some("dev.zcode.app") => "ZCode".into(),
        Some("com.microsoft.VSCode") => "Code".into(),
        Some("com.apple.Terminal") => "Terminal".into(),
        _ => String::new(),
    }
}

fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn run_osascript(script: &str) -> Result<String, String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|_| "osascript_failed".to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else if is_permission_error(&stderr) {
        Err(permission_reason(&stderr).to_string())
    } else {
        Err(format!("osascript: {stderr}"))
    }
}

fn is_permission_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("assistive access")
        || s.contains("not authorized")
        || s.contains("not allowed")
        || s.contains("-1743")
        || s.contains("-25211")
}

fn permission_reason(stderr: &str) -> &'static str {
    let s = stderr.to_ascii_lowercase();
    if s.contains("assistive access") || s.contains("-25211") {
        "needs_accessibility"
    } else {
        "needs_automation"
    }
}

thread_local! {
    static AX_FAILURE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn ax_failure_reason() -> &'static str {
    AX_FAILURE.with(|f| match f.borrow().as_deref() {
        Some(r) => match r {
            "needs_accessibility" => "needs_accessibility",
            "needs_automation" => "needs_automation",
            _ => "window_state_unavailable",
        },
        None => "window_state_unavailable",
    })
}

fn ax_window_titles(process: &str) -> Option<Vec<String>> {
    let script = format!(
        r#"tell application "System Events"
  if not (exists process "{p}") then return "__NO_PROCESS__"
  tell process "{p}"
    set out to ""
    repeat with w in windows
      try
        set out to out & (title of w as text) & "\n"
      end try
    end repeat
    return out
  end tell
end tell"#,
        p = escape_applescript(process)
    );
    match run_osascript(&script) {
        Ok(out) => {
            if out.contains("__NO_PROCESS__") {
                AX_FAILURE.with(|f| *f.borrow_mut() = Some("no_process".into()));
                return None;
            }
            Some(
                out.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
            )
        }
        Err(reason) => {
            AX_FAILURE.with(|f| *f.borrow_mut() = Some(reason));
            None
        }
    }
}

fn frontmost_app() -> Option<String> {
    let script = r#"tell application "System Events" to get name of first application process whose frontmost is true"#;
    run_osascript(script).ok().map(|s| s.trim().to_string())
}

fn raise_window(process: &str, title: &str) -> Result<(), String> {
    let script = format!(
        r#"tell application "System Events"
  tell process "{p}"
    set frontmost to true
    repeat with w in windows
      if (title of w as text) contains "{t}" then
        try
          perform action "AXRaise" of w
        end try
        try
          set value of attribute "AXMain" of w to true
        end try
        try
          set index of w to 1
        end try
        return "ok"
      end if
    end repeat
    return "not-found"
  end tell
end tell"#,
        p = escape_applescript(process),
        t = escape_applescript(title)
    );
    match run_osascript(&script) {
        Ok(out) if out == "ok" => Ok(()),
        Ok(_) => Err("window_not_found".into()),
        Err(reason) => Err(reason),
    }
}

/// Select the Terminal.app tab whose tty matches. Returns `Some(Some(tty))` on
/// success, `Some(None)` when no tab matched, `None` when Terminal could not be
/// queried (permission/automation failure).
fn select_terminal_tab(tty: &str) -> Option<Option<String>> {
    let script = format!(
        r#"tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      if (tty of t as text) is "{tty}" then
        set selected tab of w to t
        set index of w to 1
        activate
        return (tty of selected tab of w as text)
      end if
    end repeat
  end repeat
  return ""
end tell"#,
        tty = escape_applescript(tty)
    );
    match run_osascript(&script) {
        Ok(out) => {
            if out.is_empty() {
                Some(None)
            } else {
                Some(Some(out.trim().to_string()))
            }
        }
        Err(_) => {
            AX_FAILURE.with(|f| *f.borrow_mut() = Some("terminal_unavailable".into()));
            None
        }
    }
}

fn terminal_failure_reason() -> &'static str {
    AX_FAILURE.with(|f| match f.borrow().as_deref() {
        Some("needs_automation") => "needs_automation",
        Some("needs_accessibility") => "needs_accessibility",
        _ => "terminal_not_running",
    })
}

// ── Terminal tty resolution ─────────────────────────────────────

/// Find the controlling TTY of a live agent CLI process whose working
/// directory is `cwd`. Called only from a user-initiated jump, never in a scan.
pub fn resolve_terminal_tty(process_needle: &str, cwd: &str) -> Option<String> {
    if process_needle.is_empty() || cwd.is_empty() {
        return None;
    }
    let out = Command::new("ps")
        .args(["-axo", "pid=,tty=,command="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut candidates = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let mut parts = line.splitn(3, char::is_whitespace);
        let Some(pid) = parts.next() else { continue };
        let Some(tty) = parts.next() else { continue };
        let command = parts.next().unwrap_or("");
        if command.contains(process_needle) && tty.starts_with("ttys") {
            if let Ok(pid) = pid.parse::<u32>() {
                candidates.push((pid, format!("/dev/{tty}")));
            }
        }
    }
    for (pid, tty) in candidates {
        if process_cwd(pid).map(|p| p == cwd).unwrap_or(false) {
            return Some(tty);
        }
    }
    None
}

fn process_cwd(pid: u32) -> Option<String> {
    let out = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix('n').map(|s| s.to_string()))
}

// ── Pure helpers (unit tested) ──────────────────────────────────

pub fn normalize_title(s: &str) -> String {
    s.to_lowercase()
        .replace('\u{2014}', " ")
        .replace('\u{2013}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Windows whose title contains the hint (or vice versa for very short hints).
pub fn matching_windows(titles: &[String], hint: &str) -> Vec<String> {
    let h = normalize_title(hint);
    if h.is_empty() {
        return Vec::new();
    }
    let mut exact = Vec::new();
    let mut contains = Vec::new();
    for title in titles {
        let t = normalize_title(title);
        if t == h {
            exact.push(title.clone());
        } else if t.contains(&h) || (h.len() > 8 && h.contains(&t) && !t.is_empty()) {
            contains.push(title.clone());
        }
    }
    if !exact.is_empty() {
        exact
    } else {
        contains
    }
}

fn best_title_match(titles: &[String], hint: &str) -> Option<String> {
    matching_windows(titles, hint).into_iter().next()
}

fn open_url(url: &str) -> Result<(), String> {
    Command::new("open")
        .arg(url)
        .status()
        .map_err(|_| "open_failed".to_string())
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err("open_failed".into())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(level: JumpLevel, max: JumpLevel) -> TaskRecord {
        TaskRecord {
            id: "codex:x".into(),
            agent_key: "codex".into(),
            agent_name: "Codex".into(),
            host: "desktop".into(),
            session_id: Some("x".into()),
            task_id: None,
            title: "Fix the parser".into(),
            status: TaskStatus::Running,
            status_reason: "task_started".into(),
            last_activity_secs: 0,
            collected_at_secs: 0,
            summary: None,
            origin: DataOrigin::Index,
            target: JumpTarget {
                app_name: Some("ChatGPT".into()),
                bundle_id: Some("com.openai.codex".into()),
                deep_link: Some("codex://threads/x".into()),
                window_title: Some("Fix the parser".into()),
                ..Default::default()
            },
            target_validity: TargetValidity::Valid,
            invalid_reason: None,
            capability: JumpCapability {
                max_level: max,
                notes: String::new(),
                requires: vec![],
            },
            level,
            pinned: false,
        }
    }

    #[test]
    fn title_matching_is_normalized_and_exact_first() {
        let titles = vec![
            "Fix the parser — Codex".to_string(),
            "fix THE parser".to_string(),
            "Other task".to_string(),
        ];
        let m = matching_windows(&titles, "Fix the parser");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], "fix THE parser");
        assert!(!matching_windows(&titles, "unrelated").len().gt(&0));
    }

    #[test]
    fn no_hint_matches_nothing() {
        assert!(matching_windows(&["a".into()], "  ").is_empty());
    }

    #[test]
    fn invalid_target_fails_without_touching_the_app() {
        let mut t = task(JumpLevel::L2, JumpLevel::L3);
        t.target_validity = TargetValidity::Invalid;
        t.invalid_reason = Some("workspace_missing".into());
        let out = jump(&t);
        assert_eq!(out.kind, JumpOutcomeKind::Failed);
        assert_eq!(out.achieved, JumpLevel::L0);
        assert_eq!(out.reason_key, "workspace_missing");
        assert_eq!(out.next_step.as_deref(), Some("dismiss"));
    }

    #[test]
    fn no_target_fails_cleanly() {
        let mut t = task(JumpLevel::L1, JumpLevel::L1);
        t.target.bundle_id = None;
        t.target.app_name = None;
        let out = jump(&t);
        assert_eq!(out.kind, JumpOutcomeKind::Failed);
        assert_eq!(out.reason_key, "no_target");
    }

    #[test]
    fn process_name_falls_back_to_bundle() {
        let mut t = task(JumpLevel::L1, JumpLevel::L1);
        t.target.app_name = None;
        assert_eq!(process_name(&t), "ChatGPT");
        t.target.bundle_id = Some("com.deepseek.dsh".into());
        assert_eq!(process_name(&t), "DeepSeek Harness");
        t.target.bundle_id = Some("dev.zcode.app".into());
        assert_eq!(process_name(&t), "ZCode");
        t.target.bundle_id = Some("com.apple.Terminal".into());
        assert_eq!(process_name(&t), "Terminal");
    }

    #[test]
    fn applescript_escaping_blocks_injection() {
        let s = escape_applescript("a\"b\\c");
        assert_eq!(s, "a\\\"b\\\\c");
    }

    #[test]
    fn permission_errors_are_classified() {
        assert!(is_permission_error(
            "System Events got an error: osascript is not allowed assistive access."
        ));
        assert_eq!(
            permission_reason("osascript is not allowed assistive access."),
            "needs_accessibility"
        );
        assert_eq!(
            permission_reason("Not authorized to send Apple events to System Events."),
            "needs_automation"
        );
        assert!(!is_permission_error("something else"));
    }

    #[test]
    fn agent_process_needles() {
        assert_eq!(agent_process_needle("claude-code"), "claude");
        assert_eq!(agent_process_needle("codex"), "codex");
        assert_eq!(agent_process_needle("unknown"), "");
    }
}
