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
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const ACCESSIBILITY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const AUTOMATION_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation";

/// Bundle identifier of this app (`tauri.conf.json`); used to locate the
/// installed bundle when the running binary is not inside one (e.g. `cargo run`).
pub const APP_BUNDLE_ID: &str = "com.agent.monitor";

pub fn open_permission_settings(kind: PermissionKind) {
    let pane = match kind {
        PermissionKind::Accessibility => ACCESSIBILITY_PANE,
        PermissionKind::Automation => AUTOMATION_PANE,
    };
    let _ = Command::new("open").arg(pane).status();
}

// ── Diagnostics / self-check (docs/JUMP.md) ─────────────────────

/// One permission status pill in Settings → Jump.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DiagnosticStatus {
    pub granted: bool,
    /// Machine-readable reason when `granted` is false
    /// (`needs_accessibility`, `needs_automation`, `osascript_failed`,
    /// `no_running_agent_app`, `no_process`, `window_state_unavailable`, ...).
    pub detail: Option<String>,
}

/// Per-agent row of the capability table: declared maximum, install/run state
/// and deep-link support. Static capability metadata never depends on the host.
#[derive(Clone, Debug, Serialize)]
pub struct AgentDiagnostics {
    pub key: &'static str,
    pub name: &'static str,
    pub bundle_id: &'static str,
    pub app_name: &'static str,
    pub max_level: JumpLevel,
    pub notes: &'static str,
    pub installed: bool,
    pub running: bool,
    pub deep_link: bool,
}

/// `get_jump_diagnostics` payload (`docs/JUMP.md`).
#[derive(Clone, Debug, Serialize)]
pub struct JumpDiagnostics {
    pub accessibility: DiagnosticStatus,
    pub automation: DiagnosticStatus,
    pub window_access: DiagnosticStatus,
    pub system_events: bool,
    pub app_bundle_path: Option<String>,
    pub agents: Vec<AgentDiagnostics>,
}

impl JumpDiagnostics {
    /// Fallback when the blocking self-check task could not run at all.
    pub fn unavailable() -> Self {
        let failed = || DiagnosticStatus {
            granted: false,
            detail: Some("diagnostics_failed".into()),
        };
        Self {
            accessibility: failed(),
            automation: failed(),
            window_access: failed(),
            system_events: false,
            app_bundle_path: None,
            agents: Vec::new(),
        }
    }
}

/// Static per-agent map (docs/JUMP.md / docs/CAPABILITIES.md). `max_level` is
/// the declared capability, not a promise that the host can reach it today.
struct AgentSpec {
    key: &'static str,
    name: &'static str,
    bundle_id: &'static str,
    /// macOS application name as shown to the user.
    app_name: &'static str,
    /// Process name used by System Events; differs from the display name for
    /// VSCode ("Code").
    ax_process: &'static str,
    max_level: JumpLevel,
    notes: &'static str,
    deep_link: bool,
}

const AGENT_SPECS: [AgentSpec; 4] = [
    AgentSpec {
        key: "codex",
        name: "Codex",
        bundle_id: "com.openai.codex",
        app_name: "ChatGPT",
        ax_process: "ChatGPT",
        max_level: JumpLevel::L3,
        notes: "codex://threads/<id> route; L3 verified by a real window-title change",
        deep_link: true,
    },
    AgentSpec {
        key: "dsh",
        name: "DSH",
        bundle_id: "com.deepseek.dsh",
        app_name: "DeepSeek Harness",
        ax_process: "DeepSeek Harness",
        max_level: JumpLevel::L1,
        notes: "one window for all sessions; dsh://open only focuses the app (no per-session route)",
        deep_link: false,
    },
    AgentSpec {
        key: "zcode",
        name: "ZCode",
        bundle_id: "dev.zcode.app",
        app_name: "ZCode",
        ax_process: "ZCode",
        max_level: JumpLevel::L1,
        notes: "the app exits immediately on this host; activation cannot be verified",
        deep_link: false,
    },
    AgentSpec {
        key: "claude-code",
        name: "Claude Code",
        bundle_id: "com.microsoft.VSCode",
        app_name: "Visual Studio Code",
        ax_process: "Code",
        max_level: JumpLevel::L2,
        notes: "VSCode-hosted; window matched by workspace (L2 verified)",
        deep_link: false,
    },
];

/// Collect the real permission/self-check state. Blocking (spawns `osascript`,
/// `mdfind` and `ps`); call it from a background thread, never from a scan.
pub fn collect_jump_diagnostics() -> JumpDiagnostics {
    let accessibility = DiagnosticStatus {
        granted: ax_is_process_trusted(),
        detail: None,
    };
    let automation = automation_status();
    let ps_sample = sample_ps_commands();
    // System Events is the helper every AX/AppleScript read goes through. After
    // the automation probe it is usually already running; a denied grant can
    // still leave it alive, so check the process table as well.
    let system_events =
        automation.granted || ps_sample.contains("System Events.app/Contents/MacOS/");

    let agents: Vec<AgentDiagnostics> = AGENT_SPECS
        .iter()
        .map(|spec| AgentDiagnostics {
            key: spec.key,
            name: spec.name,
            bundle_id: spec.bundle_id,
            app_name: spec.app_name,
            max_level: spec.max_level,
            notes: spec.notes,
            installed: find_installed_bundle(spec.bundle_id, spec.app_name).is_some(),
            running: app_running_in(&ps_sample, spec.app_name),
            deep_link: spec.deep_link,
        })
        .collect();

    let window_access = window_access_status(&agents);

    JumpDiagnostics {
        accessibility,
        automation,
        window_access,
        system_events,
        app_bundle_path: current_app_bundle_path().map(|p| p.display().to_string()),
        agents,
    }
}

/// The window-access pill is a **real read test**: read the window titles of a
/// running agent app, exactly what L2/L3 need.
fn window_access_status(agents: &[AgentDiagnostics]) -> DiagnosticStatus {
    let Some(running) = agents.iter().find(|agent| agent.running) else {
        return DiagnosticStatus {
            granted: false,
            detail: Some("no_running_agent_app".into()),
        };
    };
    let Some(spec) = AGENT_SPECS.iter().find(|spec| spec.key == running.key) else {
        return DiagnosticStatus {
            granted: false,
            detail: Some("no_running_agent_app".into()),
        };
    };
    match ax_window_titles(spec.ax_process) {
        Some(_) => DiagnosticStatus {
            granted: true,
            detail: None,
        },
        None => DiagnosticStatus {
            granted: false,
            detail: Some(ax_failure_detail()),
        },
    }
}

/// Harmless System Events query, classified exactly like a jump would classify
/// the same failure.
fn automation_status() -> DiagnosticStatus {
    match run_osascript("tell application \"System Events\" to count processes") {
        Ok(_) => DiagnosticStatus {
            granted: true,
            detail: None,
        },
        Err(reason) => DiagnosticStatus {
            granted: false,
            detail: Some(match reason.as_str() {
                "needs_accessibility" | "needs_automation" | "osascript_failed" => reason,
                _ => "osascript_failed".into(),
            }),
        },
    }
}

/// `AXIsProcessTrusted()` from ApplicationServices (no extra crate).
#[cfg(target_os = "macos")]
fn ax_is_process_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        /// Carbon `Boolean` (unsigned char): non-zero when the process is trusted.
        fn AXIsProcessTrusted() -> u8;
    }
    // Safety: documented, side-effect-free system query.
    unsafe { AXIsProcessTrusted() != 0 }
}

#[cfg(not(target_os = "macos"))]
fn ax_is_process_trusted() -> bool {
    false
}

/// One `ps` sample shared by the install/run checks and `system_events`.
fn sample_ps_commands() -> String {
    Command::new("ps")
        .args(["-axo", "command="])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// Is the app bundle's main executable in the process table? Matching
/// `<App>.app/Contents/MacOS/` also catches the Electron main process.
fn app_running_in(ps_sample: &str, app_name: &str) -> bool {
    let needle = format!("{app_name}.app/Contents/MacOS/");
    ps_sample.lines().any(|line| line.contains(&needle))
}

/// LaunchServices (`mdfind` by bundle id) first, then the standard application
/// directories. The order is pure and unit tested.
fn find_installed_bundle(bundle_id: &str, app_name: &str) -> Option<PathBuf> {
    let mdfind_stdout = Command::new("mdfind")
        .arg(format!("kMDItemCFBundleIdentifier == '{bundle_id}'"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
    resolve_installed_bundle(mdfind_stdout.as_deref(), &app_search_dirs(), app_name)
}

fn app_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    dirs.push(PathBuf::from("/System/Applications"));
    dirs
}

/// Pure install resolution: first `.app` path from `mdfind`, then the app-name
/// fallback in the given directories (tests pass a temp dir).
fn resolve_installed_bundle(
    mdfind_stdout: Option<&str>,
    search_dirs: &[PathBuf],
    app_name: &str,
) -> Option<PathBuf> {
    if let Some(stdout) = mdfind_stdout {
        for line in stdout.lines() {
            let candidate = line.trim();
            if candidate.ends_with(".app") {
                return Some(PathBuf::from(candidate));
            }
        }
    }
    let bundle_name = format!("{app_name}.app");
    search_dirs
        .iter()
        .map(|dir| dir.join(&bundle_name))
        .find(|path| path.is_dir())
}

/// Extract the `.app` bundle from a `…/Foo.app/Contents/MacOS/binary` path.
/// `None` for `cargo run`/unbundled binaries.
pub fn app_bundle_path_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents_dir.parent()?;
    let is_app = bundle
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("app"))
        .unwrap_or(false);
    is_app.then(|| bundle.to_path_buf())
}

/// The running app bundle, or the installed copy when this is a dev binary.
fn current_app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    app_bundle_path_from_exe(&exe)
        .or_else(|| find_installed_bundle(APP_BUNDLE_ID, "Agent Monitor"))
}

/// `reveal_app_in_finder` command: reveal the `.app` so the user can drag it
/// into the Accessibility list. Never opens System Settings itself.
pub fn reveal_app_in_finder() -> Result<(), String> {
    let bundle = current_app_bundle_path().ok_or_else(|| "app_bundle_not_found".to_string())?;
    let path = bundle
        .to_str()
        .ok_or_else(|| "app_bundle_path_not_utf8".to_string())?;
    let status = Command::new("open")
        .args(["-R", path])
        .status()
        .map_err(|err| format!("open_failed: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("open_failed".into())
    }
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
        Attempt::Done(outcome) => {
            // Reporting rule: only degrade when the user asked for more than
            // the app could reach. A declared L1 adapter (DSH) succeeds at L1.
            let outcome = report_app_level(outcome, requested, first_error.as_deref());
            apply_permission_hint(outcome, first_error.as_deref())
        }
        Attempt::NotPossible(reason) => {
            let permission = permission_kind_from_reason(first_error.as_deref())
                .or_else(|| permission_kind_from_reason(Some(reason.as_str())));
            let mut outcome =
                JumpOutcome::failed(&task.id, requested, first_error.as_deref().unwrap_or(&reason));
            outcome.permission_needed = permission;
            if permission.is_some() {
                outcome.next_step = Some("grant_permission".into());
            }
            outcome
        }
    }
}

/// Reporting rule for an L1 result: app-level activation is a **degradation
/// only when `requested > achieved`**. Before this rule a declared-L1 adapter
/// (DSH) produced a permanent "degraded" toast even though it reached exactly
/// its documented maximum.
fn report_app_level(
    mut outcome: JumpOutcome,
    requested: JumpLevel,
    first_error: Option<&str>,
) -> JumpOutcome {
    if outcome.reason_key == "activated_app" && requested > outcome.achieved {
        outcome.kind = JumpOutcomeKind::Degraded;
        outcome.reason_key = first_error.unwrap_or("activated_app_only").to_string();
    }
    outcome
}

/// Propagate a missing privacy grant from the failed L3/L2 attempt to the final
/// L1 outcome, so the toast offers "Grant permission" instead of an unexplained
/// degradation/focus failure (`docs/JUMP.md`, "Jump outcome fixes").
fn apply_permission_hint(mut outcome: JumpOutcome, first_error: Option<&str>) -> JumpOutcome {
    let Some(permission) = permission_kind_from_reason(first_error) else {
        return outcome;
    };
    outcome.permission_needed = Some(permission);
    outcome.next_step = Some("grant_permission".into());
    if outcome.kind == JumpOutcomeKind::Unconfirmed {
        // "focus unconfirmed" is not actionable; the missing grant is. The
        // frontend maps `needs_accessibility` / `needs_automation` to the toast
        // text and the grant button.
        outcome.reason_key = permission_reason_key(permission).to_string();
    }
    outcome
}

/// The `reason_key` the UI already localises for a missing privacy grant.
fn permission_reason_key(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Accessibility => "needs_accessibility",
        PermissionKind::Automation => "needs_automation",
    }
}

/// Map an attempt-failure reason to the privacy grant the user must give.
fn permission_kind_from_reason(reason: Option<&str>) -> Option<PermissionKind> {
    match reason? {
        "needs_accessibility" => Some(PermissionKind::Accessibility),
        "needs_automation" => Some(PermissionKind::Automation),
        _ => None,
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
        Some("com.apple.Terminal") => "Terminal".into(),
        Some(bundle) => AGENT_SPECS
            .iter()
            .find(|spec| spec.bundle_id == bundle)
            .map(|spec| spec.ax_process.to_string())
            .unwrap_or_default(),
        None => String::new(),
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

/// Classify an AppleScript failure into the two macOS privacy grants or a
/// generic failure (pure, unit tested). `-1743` is "not authorized to send
/// Apple events" (Automation); `-25211` is a denied Accessibility prompt.
pub fn classify_osascript_error(stderr: &str) -> &'static str {
    let s = stderr.to_ascii_lowercase();
    if s.contains("assistive access") || s.contains("-25211") {
        "needs_accessibility"
    } else if s.contains("not authorized") || s.contains("not allowed") || s.contains("-1743") {
        "needs_automation"
    } else {
        "osascript_failed"
    }
}

fn is_permission_error(stderr: &str) -> bool {
    classify_osascript_error(stderr) != "osascript_failed"
}

fn permission_reason(stderr: &str) -> &'static str {
    classify_osascript_error(stderr)
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

/// Raw AX failure for diagnostics (`no_process` is useful there, while jumps
/// collapse it to `window_state_unavailable`).
fn ax_failure_detail() -> String {
    AX_FAILURE
        .with(|f| f.borrow().clone())
        .unwrap_or_else(|| "window_state_unavailable".to_string())
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

    // ── Outcome reporting rules (docs/JUMP.md "Jump outcome fixes") ──

    fn app_outcome(
        kind: JumpOutcomeKind,
        requested: JumpLevel,
        achieved: JumpLevel,
        reason_key: &str,
    ) -> JumpOutcome {
        JumpOutcome {
            task_id: "codex:x".into(),
            kind,
            requested,
            achieved,
            reason_key: reason_key.into(),
            detail: None,
            next_step: None,
            permission_needed: None,
        }
    }

    #[test]
    fn declared_l1_adapter_reports_success_at_its_maximum() {
        // DSH: requested == declared max == achieved == L1 → no degraded toast.
        let dsh = app_outcome(
            JumpOutcomeKind::Success,
            JumpLevel::L1,
            JumpLevel::L1,
            "activated_app",
        );
        let reported = report_app_level(dsh, JumpLevel::L1, Some("no_session_route"));
        assert_eq!(reported.kind, JumpOutcomeKind::Success);
        assert_eq!(reported.reason_key, "activated_app");
    }

    #[test]
    fn app_activation_below_the_requested_level_is_degraded() {
        let outcome = app_outcome(
            JumpOutcomeKind::Success,
            JumpLevel::L3,
            JumpLevel::L1,
            "activated_app",
        );
        let reported = report_app_level(outcome.clone(), JumpLevel::L3, Some("needs_accessibility"));
        assert_eq!(reported.kind, JumpOutcomeKind::Degraded);
        assert_eq!(reported.reason_key, "needs_accessibility");

        // Without a deeper error the generic reason is used.
        let generic = report_app_level(outcome, JumpLevel::L2, None);
        assert_eq!(generic.kind, JumpOutcomeKind::Degraded);
        assert_eq!(generic.reason_key, "activated_app_only");

        // Unconfirmed outcomes are not reclassified by this rule.
        let unconfirmed = app_outcome(
            JumpOutcomeKind::Unconfirmed,
            JumpLevel::L2,
            JumpLevel::L1,
            "focus_unconfirmed",
        );
        assert_eq!(
            report_app_level(unconfirmed, JumpLevel::L2, None).kind,
            JumpOutcomeKind::Unconfirmed
        );
    }

    #[test]
    fn permission_reasons_map_to_the_grant_the_user_needs() {
        assert_eq!(
            permission_kind_from_reason(Some("needs_accessibility")),
            Some(PermissionKind::Accessibility)
        );
        assert_eq!(
            permission_kind_from_reason(Some("needs_automation")),
            Some(PermissionKind::Automation)
        );
        assert_eq!(permission_kind_from_reason(Some("window_not_found")), None);
        assert_eq!(permission_kind_from_reason(None), None);
    }

    #[test]
    fn missing_grant_is_propagated_to_the_final_unconfirmed_outcome() {
        // L2/L3 failed on a missing grant and L1 could not be confirmed: the
        // toast must offer the grant action and name the permission reason.
        let unconfirmed = app_outcome(
            JumpOutcomeKind::Unconfirmed,
            JumpLevel::L3,
            JumpLevel::L1,
            "focus_unconfirmed",
        );
        let hinted = apply_permission_hint(unconfirmed, Some("needs_accessibility"));
        assert_eq!(
            hinted.permission_needed,
            Some(PermissionKind::Accessibility)
        );
        assert_eq!(hinted.next_step.as_deref(), Some("grant_permission"));
        assert_eq!(hinted.reason_key, "needs_accessibility");

        let automation = apply_permission_hint(
            app_outcome(
                JumpOutcomeKind::Unconfirmed,
                JumpLevel::L2,
                JumpLevel::L1,
                "focus_unconfirmed",
            ),
            Some("needs_automation"),
        );
        assert_eq!(automation.permission_needed, Some(PermissionKind::Automation));
        assert_eq!(automation.reason_key, "needs_automation");

        // A confirmed degradation keeps its own reason but still carries the
        // grant action (the app came forward, the window level needs the grant).
        let degraded = report_app_level(
            app_outcome(
                JumpOutcomeKind::Success,
                JumpLevel::L3,
                JumpLevel::L1,
                "activated_app",
            ),
            JumpLevel::L3,
            Some("needs_accessibility"),
        );
        let hinted = apply_permission_hint(degraded, Some("needs_accessibility"));
        assert_eq!(hinted.kind, JumpOutcomeKind::Degraded);
        assert_eq!(hinted.reason_key, "needs_accessibility");
        assert_eq!(hinted.next_step.as_deref(), Some("grant_permission"));

        // Unrelated errors are left untouched.
        let untouched = apply_permission_hint(
            app_outcome(
                JumpOutcomeKind::Unconfirmed,
                JumpLevel::L3,
                JumpLevel::L1,
                "focus_unconfirmed",
            ),
            Some("window_not_found"),
        );
        assert_eq!(untouched.permission_needed, None);
        assert_eq!(untouched.next_step, None);
        assert_eq!(untouched.reason_key, "focus_unconfirmed");
    }

    #[test]
    fn osascript_error_classification_covers_both_grants() {
        assert_eq!(
            classify_osascript_error("osascript is not allowed assistive access."),
            "needs_accessibility"
        );
        assert_eq!(
            classify_osascript_error("Not authorized to send Apple events to System Events."),
            "needs_automation"
        );
        assert_eq!(
            classify_osascript_error("execution error: System Events got an error: -1743"),
            "needs_automation"
        );
        assert_eq!(
            classify_osascript_error("some unrelated failure"),
            "osascript_failed"
        );
    }

    // ── Diagnostics helpers ─────────────────────────────────────

    #[test]
    fn app_bundle_path_is_extracted_only_from_contents_macos_binaries() {
        assert_eq!(
            app_bundle_path_from_exe(Path::new(
                "/Applications/Agent Monitor.app/Contents/MacOS/agent-monitor"
            )),
            Some(PathBuf::from("/Applications/Agent Monitor.app"))
        );
        // cargo run / unbundled binaries have no .app ancestor.
        assert_eq!(
            app_bundle_path_from_exe(Path::new("/Users/x/src-tauri/target/debug/agent-monitor")),
            None
        );
        // A helper inside Contents/Resources is not the bundle root.
        assert_eq!(
            app_bundle_path_from_exe(Path::new(
                "/Applications/Foo.app/Contents/Resources/helper"
            )),
            None
        );
        assert_eq!(app_bundle_path_from_exe(Path::new("/Applications/Foo/Contents/MacOS/x")), None);
    }

    #[test]
    fn installed_bundle_prefers_mdfind_then_falls_back_to_app_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "am-jump-installed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ChatGPT.app")).unwrap();
        let dirs = vec![dir.clone()];

        // LaunchServices wins when it reports a bundle.
        assert_eq!(
            resolve_installed_bundle(Some("/Applications/ChatGPT.app\n"), &dirs, "ChatGPT"),
            Some(PathBuf::from("/Applications/ChatGPT.app"))
        );
        // Fallback: <dir>/<App name>.app exists.
        assert_eq!(
            resolve_installed_bundle(None, &dirs, "ChatGPT"),
            Some(dir.join("ChatGPT.app"))
        );
        // Not installed anywhere we looked.
        assert_eq!(resolve_installed_bundle(None, &dirs, "ZCode"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_detection_matches_the_app_bundle_path() {
        let ps_sample = "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT --flag\n/usr/bin/foo";
        assert!(app_running_in(ps_sample, "ChatGPT"));
        assert!(!app_running_in(ps_sample, "ZCode"));
        assert!(app_running_in(
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
            "Visual Studio Code"
        ));
    }

    #[test]
    fn static_agent_map_matches_the_documented_contract() {
        let spec = |key: &str| AGENT_SPECS.iter().find(|spec| spec.key == key).unwrap();
        assert_eq!(spec("codex").bundle_id, "com.openai.codex");
        assert_eq!(spec("codex").app_name, "ChatGPT");
        assert_eq!(spec("codex").max_level, JumpLevel::L3);
        assert!(spec("codex").deep_link);

        assert_eq!(spec("dsh").bundle_id, "com.deepseek.dsh");
        assert_eq!(spec("dsh").app_name, "DeepSeek Harness");
        assert_eq!(spec("dsh").max_level, JumpLevel::L1);
        assert!(!spec("dsh").deep_link);

        assert_eq!(spec("zcode").bundle_id, "dev.zcode.app");
        assert_eq!(spec("zcode").app_name, "ZCode");
        assert!(!spec("zcode").deep_link);

        assert_eq!(spec("claude-code").bundle_id, "com.microsoft.VSCode");
        assert_eq!(spec("claude-code").app_name, "Visual Studio Code");
        assert_eq!(spec("claude-code").ax_process, "Code");
        assert_eq!(spec("claude-code").max_level, JumpLevel::L2);
        assert!(!spec("claude-code").deep_link);
    }

    #[test]
    fn diagnostics_serialise_the_documented_shape() {
        let diagnostics = JumpDiagnostics {
            accessibility: DiagnosticStatus {
                granted: false,
                detail: None,
            },
            automation: DiagnosticStatus {
                granted: true,
                detail: None,
            },
            window_access: DiagnosticStatus {
                granted: false,
                detail: Some("needs_accessibility".into()),
            },
            system_events: true,
            app_bundle_path: Some("/Applications/Agent Monitor.app".into()),
            agents: vec![AgentDiagnostics {
                key: "codex",
                name: "Codex",
                bundle_id: "com.openai.codex",
                app_name: "ChatGPT",
                max_level: JumpLevel::L3,
                notes: "codex://threads/<id>",
                installed: true,
                running: true,
                deep_link: true,
            }],
        };
        let json = serde_json::to_value(&diagnostics).unwrap();
        for key in [
            "accessibility",
            "automation",
            "window_access",
            "system_events",
            "app_bundle_path",
            "agents",
        ] {
            assert!(json.get(key).is_some(), "missing key {key} in {json}");
        }
        assert_eq!(json["accessibility"]["granted"], false);
        assert_eq!(json["automation"]["granted"], true);
        assert_eq!(json["window_access"]["detail"], "needs_accessibility");
        for key in [
            "key",
            "name",
            "bundle_id",
            "app_name",
            "max_level",
            "notes",
            "installed",
            "running",
            "deep_link",
        ] {
            assert!(
                json["agents"][0].get(key).is_some(),
                "missing agent key {key} in {}",
                json["agents"][0]
            );
        }
        assert_eq!(json["agents"][0]["max_level"], "L3");
    }
}
