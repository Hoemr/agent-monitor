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

/// `JumpTarget::host_ref` marker for the DSH sidebar session route (L3).
pub const DSH_SIDEBAR_HOST_REF: &str = "dsh-sidebar";
const DSH_APP_NAME: &str = "DeepSeek Harness";
const DSH_BUNDLE_ID: &str = "com.deepseek.dsh";
const DSH_PROCESS_NEEDLE: &str = "DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness";
const VSCODE_BUNDLE_ID: &str = "com.microsoft.VSCode";
const VSCODE_APP_NAME: &str = "Code";
const VSCODE_PROCESS_NEEDLE: &str = "Visual Studio Code.app/Contents/MacOS/";
/// A session row must share at least this many characters with the stored title
/// for a prefix match (exact matches are always accepted).
const MIN_TITLE_PREFIX_CHARS: usize = 6;
/// Click/verify timings (real macOS app switches are asynchronous).
const DSH_FORWARD_SETTLE_MS: u64 = 350;
/// First click + at most one retry from a freshly re-read row (re-layout can
/// make a single synthetic click a no-op).
const DSH_CLICK_ATTEMPTS: u32 = 2;
/// Bounded-verification polling: probe every 120 ms for up to ~2 s instead of
/// sleeping once and declaring failure. macOS activation and Electron session
/// switches are asynchronous; a single short sleep produced false
/// `focus_unconfirmed` / `deep_link_unverified` / `session_click_unverified`.
const POLL_INTERVAL_MS: u64 = 120;
const VERIFY_TIMEOUT_MS: u64 = 2_000;
/// Deep-link navigation is the longest part of a successful Codex jump; if the
/// host never navigates (deleted thread) there is no point waiting the full
/// general timeout before reporting the honest degradation.
const DEEP_LINK_NAV_TIMEOUT_MS: u64 = 1_200;
/// DSH session switch after a click, and workspace-group expansion render.
const DSH_TITLE_TIMEOUT_MS: u64 = 1_500;
const DSH_EXPAND_TIMEOUT_MS: u64 = 1_500;

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
        max_level: JumpLevel::L3,
        notes: "sidebar session selection via macOS AX click; verified by window-title read-back; requires Accessibility",
        deep_link: true,
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
        max_level: JumpLevel::L3,
        notes: "VSCode /open?session=<id>; verify the focused chat identity before claiming L3",
        deep_link: true,
    },
];

/// Collect the real permission/self-check state. Blocking (spawns `osascript`,
/// `mdfind` and `ps`); call it from a background thread, never from a scan.
#[cfg(target_os = "windows")]
pub fn collect_jump_diagnostics() -> JumpDiagnostics { crate::windows::diagnostics() }

#[cfg(not(target_os = "windows"))]
pub fn collect_jump_diagnostics() -> JumpDiagnostics {
    let accessibility = DiagnosticStatus {
        granted: crate::ax::is_trusted(),
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

// ── TCC permission repair (`reset_app_permissions`) ─────────────

/// Outcome of one `tccutil reset` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TccReset {
    /// The record existed and was cleared (or tccutil reported success).
    Reset,
    /// No TCC record for this bundle id: nothing to clear (not a failure).
    Skipped,
    /// tccutil failed for another reason (message preserved for the UI).
    Error(String),
}

/// Classify `tccutil reset <service> <bundle>` output (pure, unit tested).
///
/// A missing record is the normal case on a fresh/ad-hoc-signed copy: macOS
/// prints `No such bundle identifier …` and exits non-zero. It is a *skip*, not
/// an error; the user must re-add the app either way.
pub fn classify_tccutil_result(success: bool, output: &str) -> TccReset {
    let text = output.to_ascii_lowercase();
    if text.contains("no such bundle identifier")
        || text.contains("no such bundle")
        || text.contains("not found")
        || text.contains("does not exist")
    {
        return TccReset::Skipped;
    }
    if success {
        return TccReset::Reset;
    }
    let detail = output.trim();
    TccReset::Error(if detail.is_empty() {
        "tccutil_failed".to_string()
    } else {
        detail.to_string()
    })
}

/// Build the exact `tccutil reset` argv. The bundle id is MANDATORY and always
/// the last argument: `tccutil reset <service>` without it clears that service
/// for EVERY app on the machine (never do that). This builder is the only way
/// the reset path constructs the command.
fn tccutil_reset_argv(service: &str, bundle_id: &str) -> Result<Vec<String>, String> {
    let service = service.trim();
    let bundle_id = bundle_id.trim();
    if service.is_empty() {
        return Err("tccutil_service_required".to_string());
    }
    if bundle_id.is_empty() {
        return Err("tccutil_bundle_id_required".to_string());
    }
    Ok(vec![
        "/usr/bin/tccutil".to_string(),
        "reset".to_string(),
        service.to_string(),
        bundle_id.to_string(),
    ])
}

fn format_reset_message(reset: &[&str], skipped: &[&str], zh: bool) -> String {
    let none = if zh { "无" } else { "none" };
    let reset_list = if reset.is_empty() {
        none.to_string()
    } else {
        reset.join(if zh { "、" } else { ", " })
    };
    let skipped_list = if skipped.is_empty() {
        none.to_string()
    } else {
        skipped.join(if zh { "、" } else { ", " })
    };
    if zh {
        format!(
            "已重置：{reset_list}。无记录（无需重置）：{skipped_list}。\
             仅清除了 {APP_BUNDLE_ID} 的记录，其它应用不受影响。\
             请通过「设置 → 跳转 → 在访达中显示」把当前 Agent Monitor.app 重新加入\
             「系统设置 → 隐私与安全性 → 辅助功能 / 自动化」，然后重启 Agent Monitor \
             （旧进程在重启前可能仍无权限）。"
        )
    } else {
        format!(
            "Reset: {reset_list}. No record (nothing to reset): {skipped_list}. \
             Only records for {APP_BUNDLE_ID} were cleared; other apps are untouched. \
             Re-add the current Agent Monitor.app via Settings → Jump → Reveal App in Finder, \
             then restart Agent Monitor (the running process may stay untrusted until relaunch)."
        )
    }
}

fn format_reset_errors(errors: &[String], zh: bool) -> String {
    if zh {
        format!("部分服务重置失败：{}。", errors.join("；"))
    } else {
        format!("Some services could not be reset: {}.", errors.join("; "))
    }
}

fn run_tccutil_reset(service: &str, bundle_id: &str) -> TccReset {
    let argv = match tccutil_reset_argv(service, bundle_id) {
        Ok(argv) => argv,
        // Never fall back to a bundle-less reset: it would clear every app.
        Err(err) => return TccReset::Error(err),
    };
    match Command::new(&argv[0]).args(&argv[1..]).output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            classify_tccutil_result(output.status.success(), &text)
        }
        Err(err) => TccReset::Error(format!("tccutil unavailable: {err}")),
    }
}

fn system_is_chinese() -> bool {
    static ZH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ZH.get_or_init(|| {
        Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("zh")
            })
            .unwrap_or(false)
    })
}

/// `reset_app_permissions` command: clear this bundle's stale TCC records so the
/// user can re-add the *current* copy. Never called automatically (the Settings
/// UI asks for inline confirmation first); never requires sudo.
///
/// Returns a human-readable summary (reset/skipped services + re-add/restart
/// instructions), or an error when a service could not be reset.
pub fn reset_app_permissions() -> Result<String, String> {
    let zh = system_is_chinese();
    let services: [(&str, &str, &str); 2] = [
        ("Accessibility", "Accessibility", "辅助功能"),
        ("AppleEvents", "automation (AppleEvents)", "自动化（Apple 事件）"),
    ];
    let mut reset: Vec<&str> = Vec::new();
    let mut skipped: Vec<&str> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (service, english_label, chinese_label) in services {
        let label = if zh { chinese_label } else { english_label };
        match run_tccutil_reset(service, APP_BUNDLE_ID) {
            TccReset::Reset => reset.push(label),
            TccReset::Skipped => skipped.push(label),
            TccReset::Error(detail) => errors.push(format!("{label}: {detail}")),
        }
    }
    let message = format_reset_message(&reset, &skipped, zh);
    if errors.is_empty() {
        Ok(message)
    } else {
        Err(format!("{message} {}", format_reset_errors(&errors, zh)))
    }
}


// ── Public entry ────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub fn jump(task: &TaskRecord, _peers: &[TaskRecord]) -> JumpOutcome { crate::windows::jump(task) }

#[cfg(not(target_os = "windows"))]
pub fn jump(task: &TaskRecord, peers: &[TaskRecord]) -> JumpOutcome {
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

    // L3 — session / tab level. A deep link or a host-specific session route
    // (DSH sidebar AX row) is required; otherwise the ladder starts at L2.
    if max >= JumpLevel::L3 && has_session_route(task) {
        let unique_title = peers.iter().filter(|peer| peer.target.bundle_id == task.target.bundle_id && peer.title == task.title).count() == 1;
        match try_session_level(task, unique_title) {
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

fn has_session_route(task: &TaskRecord) -> bool {
    // Terminal tab selection (kept unchanged: it has its own L3 path).
    task.target.terminal_host.as_deref() == Some("Terminal")
        || task.target.deep_link.is_some()
        || task.target.host_ref.as_deref() == Some(DSH_SIDEBAR_HOST_REF)
}

fn try_session_level(task: &TaskRecord, unique_title: bool) -> Attempt {
    if task.target.bundle_id.as_deref() == Some(VSCODE_BUNDLE_ID) {
        return try_vscode_session(task, unique_title);
    }
    // Terminal tab selection (Claude Code CLI and any terminal-hosted agent).
    if let Some(host) = task.target.terminal_host.as_deref() {
        if host == "Terminal" {
            return try_terminal_tab(task);
        }
    }
    // DSH: single-window Electron host, but its sidebar session rows are AX
    // elements and a real click switches the session. Verified by window title.
    if task.target.host_ref.as_deref() == Some(DSH_SIDEBAR_HOST_REF) {
        return try_dsh_sidebar(task);
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
    // Slow activations are normal: poll for the host to come forward instead of
    // one fixed sleep (the old code warned `deep_link_unverified` on slow hosts).
    let activated = poll_until(DEEP_LINK_NAV_TIMEOUT_MS, || {
        (frontmost_app().as_deref() == Some(process.as_str())).then_some(())
    });
    if activated.is_none() {
        return Attempt::NotPossible("deep_link_unverified".into());
    }
    // Wait for real navigation evidence: a new window title, or the expected
    // title becoming visible. Report only after the bounded timeout expires.
    let navigated = poll_until(DEEP_LINK_NAV_TIMEOUT_MS, || {
        let titles = ax_window_titles(&process)?;
        let newly_visible = titles
            .iter()
            .find(|title| !before_titles.contains(title))
            .cloned();
        if newly_visible.is_some() {
            return newly_visible;
        }
        if expected_title.is_empty() {
            return None;
        }
        best_title_match(&titles, expected_title)
    });
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
    // The title poll expired: if AX itself is denied, say so instead of a vague
    // "deep link unverified"; the grant hint is attached by the L1 outcome.
    match ax_failure_reason() {
        "needs_accessibility" => return Attempt::NotPossible("needs_accessibility".into()),
        "needs_automation" => return Attempt::NotPossible("needs_automation".into()),
        _ => {}
    }
    // Codex window titles are AI-generated and never match the stored prompt,
    // so L2 title matching is a guaranteed 2 s timeout for this host. Skip it
    // and let the honest L1 outcome carry `deep_link_unverified`.
    if task.target.bundle_id.as_deref() == Some("com.openai.codex") {
        return Attempt::NotPossible("deep_link_unverified".into());
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
    // VSCode: project-precise focusing via the `code` CLI comes first; the AX
    // title/raise path below remains as the fallback.
    if task.target.bundle_id.as_deref() == Some(VSCODE_BUNDLE_ID) {
        if let Some(workspace) = task
            .target
            .workspace_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
        {
            if let Attempt::Done(outcome) = try_vscode_reuse_window(task, workspace) {
                return Attempt::Done(outcome);
            }
        }
    }
    let process = process_name(task);
    let Some(expected) = task.target.window_title.as_deref().filter(|s| !s.is_empty()) else {
        return Attempt::NotPossible("no_window_hint".into());
    };
    // Bounded poll: AX may still be building the window list right after an
    // activation. Ambiguity is only accepted once the matching list has been
    // stable across two reads (or the timeout has really expired).
    let mut last_matches: Option<Vec<String>> = None;
    let mut previous: Option<Vec<String>> = None;
    let matched = poll_until(VERIFY_TIMEOUT_MS, || {
        let titles = ax_window_titles(&process)?;
        let matches = matching_windows(&titles, expected);
        if matches.is_empty() {
            previous = None;
            return None;
        }
        if previous.as_ref() == Some(&matches) {
            return Some(matches.clone());
        }
        previous = Some(matches.clone());
        last_matches = Some(matches);
        None
    });
    let Some(matches) = matched.or(last_matches) else {
        return Attempt::NotPossible(match ax_failure_reason() {
            "needs_accessibility" => "needs_accessibility",
            "needs_automation" => "needs_automation",
            _ => "window_not_found",
        }
        .into());
    };
    let ambiguous = matches.len() > 1;
    let chosen = matches[0].clone();
    match raise_window(&process, &chosen) {
        Ok(()) => {}
        Err(reason) => return Attempt::NotPossible(reason),
    }
    let verified = poll_until(VERIFY_TIMEOUT_MS, || {
        (frontmost_app().as_deref() == Some(process.as_str())).then_some(())
    })
    .is_some();
    let achieved = JumpLevel::L2;
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

/// VSCode project-precise window jump (L2).
///
/// Open the workspace before delivering the extension URI. A visible window
/// alone cannot prove that the requested chat was selected.
fn try_vscode_session(task: &TaskRecord, unique_title: bool) -> Attempt {
    let (Some(workspace), Some(link), Some(id)) = (
        task.target.workspace_path.as_deref(), task.target.deep_link.as_deref(), task.session_id.as_deref(),
    ) else { return Attempt::NotPossible("no_session_route".into()); };
    if let Attempt::NotPossible(reason) = try_vscode_reuse_window(task, workspace) {
        return Attempt::NotPossible(reason);
    }
    if let Err(reason) = open_url(link) { return Attempt::NotPossible(reason); }
    let matched = poll_until(DEEP_LINK_NAV_TIMEOUT_MS, || {
        let context = crate::ax::focused_conversation(&[VSCODE_BUNDLE_ID.to_string()])?;
        let identified = context.references.iter().any(|reference| reference_has_session(reference, id));
        let titled = unique_title && !task.title.trim().is_empty()
            && context.focused_web_title.as_deref() == Some(task.title.as_str());
        (identified || titled).then_some(context.title)
    });
    if let Some(title) = matched {
        Attempt::Done(JumpOutcome {
            task_id: task.id.clone(), kind: JumpOutcomeKind::Success,
            requested: task.level, achieved: JumpLevel::L3,
            reason_key: "opened_session".into(), detail: Some(title),
            next_step: None, permission_needed: None,
        })
    } else {
        Attempt::NotPossible("deep_link_unverified".into())
    }
}

fn reference_has_session(reference: &str, id: &str) -> bool {
    !id.is_empty() && reference.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').any(|part| part == id)
}

fn try_vscode_reuse_window(task: &TaskRecord, workspace: &str) -> Attempt {
    let Some(basename) = workspace_basename(workspace) else {
        return Attempt::NotPossible("no_window_hint".into());
    };
    let launched = match resolve_code_cli(
        std::env::var("PATH").ok().as_deref(),
        &code_cli_extra_dirs(),
    ) {
        Some(cli) => Command::new(cli)
            .args(["--reuse-window", workspace])
            .status(),
        None => Command::new("open")
            .args(["-b", VSCODE_BUNDLE_ID, workspace])
            .status(),
    };
    if !launched.map(|status| status.success()).unwrap_or(false) {
        return Attempt::NotPossible("window_not_found".into());
    }
    // The CLI returns before the window is focused; poll instead of one sleep.
    let focused = poll_until(VERIFY_TIMEOUT_MS, || {
        (frontmost_app().as_deref() == Some(VSCODE_APP_NAME)).then_some(())
    });
    if focused.is_none() {
        return Attempt::NotPossible("focus_unconfirmed".into());
    }
    let Some(pid) = app_pid(VSCODE_PROCESS_NEEDLE) else {
        return Attempt::NotPossible("focus_unconfirmed".into());
    };
    let title = poll_until(VERIFY_TIMEOUT_MS, || {
        let title = crate::ax::focused_window_title(pid)?;
        window_title_contains_workspace(&title, &basename).then_some(title)
    });
    let Some(title) = title else {
        return Attempt::NotPossible("window_not_found".into());
    };
    Attempt::Done(JumpOutcome {
        task_id: task.id.clone(),
        kind: JumpOutcomeKind::Success,
        requested: task.level,
        achieved: JumpLevel::L2,
        reason_key: "opened_window".into(),
        detail: Some(title),
        next_step: None,
        permission_needed: None,
    })
}

/// DSH sidebar session jump (L3).
///
/// Evidence chain: AX read of the sidebar rows → score every session row on
/// title/project/relative-time → real CGEvent click on the matched title →
/// focused window title must read back as "<title> — DeepSeek Harness".
fn try_dsh_sidebar(task: &TaskRecord) -> Attempt {
    if !crate::ax::is_trusted() {
        return Attempt::NotPossible("needs_accessibility".into());
    }
    let app_name = task.target.app_name.as_deref().unwrap_or(DSH_APP_NAME);
    let expected_title = task
        .target
        .window_title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&task.title);

    // Launch/focus the host if needed: the AX tree and the click target a live
    // window.
    if dsh_pid().is_none() {
        let _ = Command::new("open").args(["-b", DSH_BUNDLE_ID]).status();
        std::thread::sleep(Duration::from_millis(900));
    }
    let pid = match dsh_pid() {
        Some(pid) => pid,
        None => {
            std::thread::sleep(Duration::from_millis(700));
            match dsh_pid() {
                Some(pid) => pid,
                None => return Attempt::NotPossible("app_not_found".into()),
            }
        }
    };

    // Bring DSH forward *before* reading row frames: activation may re-layout or
    // scroll the sidebar, and a click into a background app only activates it.
    let _ = Command::new("open").args(["-b", DSH_BUNDLE_ID]).status();
    std::thread::sleep(Duration::from_millis(DSH_FORWARD_SETTLE_MS));

    // Already on the session: window-title read-back is the strongest evidence,
    // no click needed.
    if let Some(focused) = crate::ax::focused_window_title(pid) {
        if session_title_matches(&focused, app_name, expected_title) {
            return Attempt::Done(session_jump_outcome(task, focused));
        }
    }

    let debug = std::env::var_os("AGENT_MONITOR_DEBUG_JUMP").is_some();
    // Activation makes Chromium re-layout the sidebar (transient 1 px AX
    // frames even after a short settle), so wait for a sane, stable row.
    let (_session, target_rect) =
        match stable_session_target(pid, task, crate::model::now_secs(), debug) {
            Ok(target) => target,
            Err(reason) if reason == "session_row_not_found" => {
                // DSH renders sessions only for the ACTIVE workspace; sessions in
                // other projects live behind collapsed workspace rows. Expand the
                // task's project group once, then retry the normal selection.
                match expand_workspace_for_task(pid, task, debug) {
                    Ok(()) => {
                        match stable_session_target(pid, task, crate::model::now_secs(), debug) {
                            Ok(target) => target,
                            Err(reason) => return Attempt::NotPossible(reason),
                        }
                    }
                    Err(reason) => return Attempt::NotPossible(reason),
                }
            }
            // Never click a guessed row: an ambiguous match degrades honestly.
            Err(reason) => return Attempt::NotPossible(reason),
        };
    if debug {
        eprintln!(
            "dsh-jump: selected={:?} frame={:?} focused_before={:?}",
            _session.title,
            target_rect,
            crate::ax::focused_window_title(pid)
        );
    }
    click_dsh_session(pid, task, app_name, expected_title, target_rect, debug)
}

/// Click one DSH session row and verify the focused window title, polling for
/// the asynchronous session switch. One retry from a freshly re-read frame is
/// allowed; `session_click_unverified` is only reported after that.
fn click_dsh_session(
    pid: u32,
    task: &TaskRecord,
    app_name: &str,
    expected_title: &str,
    mut target_rect: crate::ax::Rect,
    debug: bool,
) -> Attempt {
    for attempt in 1..=DSH_CLICK_ATTEMPTS {
        // Click immediately: frames are only valid for the state just observed.
        if !crate::ax::click(target_rect) {
            return Attempt::NotPossible("session_click_unverified".into());
        }
        // Electron session switches are asynchronous; poll the focused title
        // instead of one fixed sleep (old code warned on slow switches).
        let verified = poll_until(DSH_TITLE_TIMEOUT_MS, || {
            crate::ax::focused_window_title(pid)
                .filter(|focused| session_title_matches(focused, app_name, expected_title))
        });
        if debug {
            eprintln!(
                "dsh-jump: attempt {attempt} focused_after={:?}",
                crate::ax::focused_window_title(pid)
            );
        }
        if let Some(focused) = verified {
            return Attempt::Done(session_jump_outcome(task, focused));
        }
        if attempt >= DSH_CLICK_ATTEMPTS {
            return Attempt::NotPossible("session_click_unverified".into());
        }
        // The click may have been swallowed by a re-layout: re-read the rows and
        // retry once from the *new* frame. Ambiguity still means "do not click".
        let fresh = read_fresh_selection(pid, task, crate::model::now_secs());
        match plan_retry(attempt, DSH_CLICK_ATTEMPTS, fresh) {
            RetryPlan::Retry { title, frame } => {
                if debug {
                    eprintln!("dsh-jump: retry click title={title:?} frame={frame:?}");
                }
                target_rect = frame;
            }
            RetryPlan::GiveUp(reason) => return Attempt::NotPossible(reason.to_string()),
        }
    }
    Attempt::NotPossible("session_click_unverified".into())
}

/// Workspace/project group row of the DSH sidebar (one-text row).
#[derive(Clone, Debug, PartialEq)]
struct SidebarWorkspace {
    title: String,
    title_frame: Option<crate::ax::Rect>,
    row_frame: Option<crate::ax::Rect>,
    /// True when session rows follow this workspace row before the next
    /// workspace (the group is rendered open). Clicking an open group would
    /// collapse it, so it is never used for expansion.
    expanded: bool,
}

/// A one-text sidebar row is a workspace/project group row; the following
/// multi-text rows are that workspace's sessions.
fn parse_sidebar_workspaces(rows: &[crate::ax::Row]) -> Vec<SidebarWorkspace> {
    let mut workspaces: Vec<SidebarWorkspace> = Vec::new();
    for row in rows {
        let texts: Vec<&crate::ax::TextRun> = row
            .texts
            .iter()
            .filter(|run| !run.text.trim().is_empty())
            .collect();
        if texts.is_empty() {
            continue;
        }
        if texts.len() == 1 {
            workspaces.push(SidebarWorkspace {
                title: texts[0].text.trim().to_string(),
                title_frame: texts[0].frame,
                row_frame: row.frame,
                expanded: false,
            });
        } else if let Some(current) = workspaces.last_mut() {
            current.expanded = true;
        }
    }
    workspaces
}

/// Pick the workspace row to expand: exact normalised match first, then a
/// unique "contains" match (both directions). More than one candidate at the
/// winning level is ambiguous and yields `None` — a guessed row is never
/// clicked. Expanded groups are excluded: clicking them would collapse them.
fn choose_workspace<'a>(
    workspaces: &'a [SidebarWorkspace],
    project_basename: &str,
) -> Option<&'a SidebarWorkspace> {
    let expected = normalize_title(project_basename);
    if expected.is_empty() {
        return None;
    }
    let collapsed: Vec<&SidebarWorkspace> = workspaces
        .iter()
        .filter(|workspace| !workspace.expanded)
        .collect();
    let exact: Vec<&SidebarWorkspace> = collapsed
        .iter()
        .copied()
        .filter(|workspace| normalize_title(&workspace.title) == expected)
        .collect();
    match exact.len() {
        1 => return Some(exact[0]),
        // Duplicate project names: do not guess.
        n if n > 1 => return None,
        _ => {}
    }
    let contains: Vec<&SidebarWorkspace> = collapsed
        .iter()
        .copied()
        .filter(|workspace| {
            let actual = normalize_title(&workspace.title);
            !actual.is_empty() && (actual.contains(&expected) || expected.contains(&actual))
        })
        .collect();
    if contains.len() == 1 {
        Some(contains[0])
    } else {
        None
    }
}

/// Expand the collapsed workspace group that owns the task's project, then wait
/// for the target session row to be rendered. Returns `project_not_found` when
/// no unique workspace row can be picked, and `session_row_not_found` when the
/// group expands but the session is still absent from the root sidebar.
fn expand_workspace_for_task(pid: u32, task: &TaskRecord, debug: bool) -> Result<(), String> {
    let Some(basename) = task
        .target
        .workspace_path
        .as_deref()
        .and_then(workspace_basename)
    else {
        return Err("project_not_found".into());
    };
    let rows = crate::ax::rows(pid);
    if rows.is_empty() {
        return Err("ax_unavailable".into());
    }
    let workspaces = parse_sidebar_workspaces(&rows);
    let Some(workspace) = choose_workspace(&workspaces, &basename) else {
        return Err("project_not_found".into());
    };
    // The workspace was already open, so the target session is genuinely not in
    // the rendered list (e.g. a subagent session): never collapse the group.
    if workspace.expanded {
        return Err("session_row_not_found".into());
    }
    let Some(rect) = workspace.title_frame.or(workspace.row_frame) else {
        return Err("project_not_found".into());
    };
    if !rect.is_sane() || rect.h < 10.0 || rect.w < 10.0 {
        return Err("project_not_found".into());
    }
    if debug {
        eprintln!(
            "dsh-jump: expanding workspace {:?} at {:?}",
            workspace.title, rect
        );
    }
    if !crate::ax::click(rect) {
        return Err("session_row_not_found".into());
    }
    // Expanding selects the workspace's last session and re-renders the group;
    // poll until the target session row exists before trying to click it.
    let now = crate::model::now_secs();
    match poll_until(DSH_EXPAND_TIMEOUT_MS, || {
        match read_fresh_selection(pid, task, now) {
            FreshSelection::Match { .. } => Some(Ok(())),
            FreshSelection::Ambiguous => Some(Err("session_ambiguous".to_string())),
            FreshSelection::None => None,
        }
    }) {
        Some(result) => result,
        None => Err("session_row_not_found".into()),
    }
}

/// Snapshot of a fresh post-miss read used by [`plan_retry`].
#[derive(Clone, Debug, PartialEq)]
enum FreshSelection {
    None,
    Ambiguous,
    Match {
        title: String,
        frame: Option<crate::ax::Rect>,
    },
}

/// What to do after a click that did not verify: one more click is allowed only
/// when the fresh read still nominates exactly one matched row with a sane text
/// frame and the attempt budget is not exhausted. A guessed row is never clicked.
#[derive(Clone, Debug, PartialEq)]
enum RetryPlan {
    Retry {
        title: String,
        frame: crate::ax::Rect,
    },
    GiveUp(&'static str),
}

fn plan_retry(attempts_used: u32, max_attempts: u32, fresh: FreshSelection) -> RetryPlan {
    if attempts_used >= max_attempts {
        return RetryPlan::GiveUp("session_click_unverified");
    }
    match fresh {
        FreshSelection::None => RetryPlan::GiveUp("session_row_not_found"),
        FreshSelection::Ambiguous => RetryPlan::GiveUp("session_ambiguous"),
        FreshSelection::Match {
            title,
            frame: Some(frame),
        } if frame.is_sane() && frame.h >= 10.0 && frame.w >= 10.0 => {
            RetryPlan::Retry { title, frame }
        }
        // A 1 px frame means "layout in progress": never click it.
        FreshSelection::Match { .. } => RetryPlan::GiveUp("session_click_unverified"),
    }
}

/// One fresh sidebar read after a click miss (no stability polling: the app is
/// already active and this is the last chance before an honest failure).
fn read_fresh_selection(pid: u32, task: &TaskRecord, now_secs: u64) -> FreshSelection {
    let rows = crate::ax::rows(pid);
    if rows.is_empty() {
        return FreshSelection::None;
    }
    let sessions = parse_sidebar_sessions(&rows);
    match select_session(&sessions, task, now_secs) {
        SessionSelection::None => FreshSelection::None,
        SessionSelection::Ambiguous => FreshSelection::Ambiguous,
        SessionSelection::One(session) => FreshSelection::Match {
            title: session.title.clone(),
            frame: session.title_frame.or(session.row_frame),
        },
    }
}

fn session_jump_outcome(task: &TaskRecord, detail: String) -> JumpOutcome {
    JumpOutcome {
        task_id: task.id.clone(),
        kind: JumpOutcomeKind::Success,
        requested: task.level,
        achieved: JumpLevel::L3,
        reason_key: "opened_session".into(),
        detail: Some(detail),
        next_step: None,
        permission_needed: None,
    }
}

fn app_pid(needle: &str) -> Option<u32> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .ok()?;
    pid_for_app_path(&String::from_utf8_lossy(&output.stdout), needle)
}

fn dsh_pid() -> Option<u32> {
    app_pid(DSH_PROCESS_NEEDLE)
}

/// Read the AX sidebar until the selected session row is sane and stable across
/// two consecutive reads, then return it with its click target. Chromium
/// re-lays out the sidebar on activation, so a single read can see 1 px frames
/// or a pre-activation ordering; clicking those would hit the wrong row.
fn stable_session_target(
    pid: u32,
    task: &TaskRecord,
    now_secs: u64,
    debug: bool,
) -> Result<(SidebarSession, crate::ax::Rect), String> {
    const ATTEMPTS: u32 = 12;
    /// How many reads a transient "no match"/"ambiguous" may last after
    /// activation before it is reported (the sidebar is re-laid out on focus).
    const SELECTION_SETTLE_ATTEMPTS: u32 = 8;
    let mut previous: Option<(String, crate::ax::Rect)> = None;
    let mut last_sane: Option<(SidebarSession, crate::ax::Rect)> = None;
    for attempt in 0..ATTEMPTS {
        let rows = crate::ax::rows(pid);
        if rows.is_empty() {
            std::thread::sleep(Duration::from_millis(150));
            continue;
        }
        let sessions = parse_sidebar_sessions(&rows);
        let session = match select_session(&sessions, task, now_secs) {
            SessionSelection::One(session) => session,
            // A miss right after activation is usually transient: Chromium may
            // have rendered only the workspace rows (or half the list) yet.
            // Retry a bounded number of times before declaring the session
            // absent/ambiguous, so a fast click never degrades spuriously.
            SessionSelection::None => {
                if attempt + 1 < SELECTION_SETTLE_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
                return Err("session_row_not_found".into());
            }
            SessionSelection::Ambiguous => {
                if attempt + 1 < SELECTION_SETTLE_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
                return Err("session_ambiguous".into());
            }
        };
        let Some(rect) = session.title_frame.or(session.row_frame) else {
            std::thread::sleep(Duration::from_millis(150));
            continue;
        };
        // A real text run is ~12-18 pt high; 1 px means "layout in progress".
        if !rect.is_sane() || rect.h < 10.0 || rect.w < 10.0 {
            std::thread::sleep(Duration::from_millis(150));
            continue;
        }
        let title = session.title.clone();
        let stable = previous
            .as_ref()
            .map(|(previous_title, previous_rect)| {
                *previous_title == title
                    && (previous_rect.x - rect.x).abs() < 1.5
                    && (previous_rect.y - rect.y).abs() < 1.5
            })
            .unwrap_or(false);
        if stable {
            if debug {
                eprintln!("dsh-jump: row stable after {} attempt(s)", attempt + 1);
            }
            return Ok((session.clone(), rect));
        }
        previous = Some((title, rect));
        last_sane = Some((session.clone(), rect));
        std::thread::sleep(Duration::from_millis(150));
    }
    // Layout never settled completely: use the last sane observation (still far
    // better than a 1 px frame); the click is verified afterwards.
    last_sane.ok_or_else(|| "ax_unavailable".to_string())
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
    // Slow launch/activation: bound the wait instead of one fixed sleep, and
    // only report `unconfirmed` after the poll really expires.
    let confirmed = poll_until(VERIFY_TIMEOUT_MS, || {
        (frontmost_app().as_deref() == Some(process.as_str())).then_some(())
    })
    .is_some();
    if confirmed {
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

/// Read the selected tab only while Terminal is the foreground application.
/// No tab switching or app activation; a denied Automation grant returns None.
pub fn focused_terminal_tty() -> Option<String> {
    if crate::ax::foreground_bundle_id().as_deref() != Some("com.apple.Terminal") { return None; }
    let tty = run_osascript(r#"with timeout of 2 seconds
      tell application "Terminal"
        if (count of windows) is 0 then return ""
        return tty of selected tab of front window
      end tell
    end timeout"#).ok()?;
    let tty = tty.trim();
    let suffix = tty.strip_prefix("/dev/ttys")?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) { return None; }
    (crate::ax::foreground_bundle_id().as_deref() == Some("com.apple.Terminal")).then(|| tty.to_owned())
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

// ── Bounded verification polling ────────────────────────────────

/// Probe `probe` immediately and then every [`POLL_INTERVAL_MS`] until it
/// returns `Some` or `timeout_ms` expires. Used for every asynchronous macOS
/// signal (frontmost app, window title, AX window list, DSH session switch) so
/// a slow-but-successful jump is never reported as unconfirmed.
fn poll_until<T>(timeout_ms: u64, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    loop {
        if let Some(value) = probe() {
            return Some(value);
        }
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return None;
        }
        let remaining_ms = timeout.saturating_sub(elapsed).as_millis() as u64;
        std::thread::sleep(Duration::from_millis(
            POLL_INTERVAL_MS.min(remaining_ms.max(1)),
        ));
    }
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

// ── DSH sidebar matching helpers (pure, unit tested) ───────────

/// One parsed sidebar session row (see docs/ATTENTION.md-adjacent jump notes).
#[derive(Clone, Debug, PartialEq)]
struct SidebarSession {
    /// Workspace/project row text that preceded this session row.
    workspace: Option<String>,
    title: String,
    /// Frame of the title text run (best click target).
    title_frame: Option<crate::ax::Rect>,
    /// Frame of the whole row (fallback click target).
    row_frame: Option<crate::ax::Rect>,
    status: Option<String>,
    /// Relative time label from the row, e.g. "23分钟" -> (1380, 60).
    time_secs_ago: Option<u64>,
    time_unit_secs: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SessionSelection<'a> {
    None,
    Ambiguous,
    One(&'a SidebarSession),
}

/// Status words DSH renders next to a sidebar session (exact matches only, so a
/// title that merely contains "完成" is not discarded).
fn is_status_label(text: &str) -> bool {
    let trimmed = text.trim();
    const CHINESE: [&str; 10] = [
        "进行中",
        "已完成",
        "失败",
        "已中止",
        "已取消",
        "等待中",
        "等待审批",
        "等待输入",
        "运行中",
        "空闲",
    ];
    if CHINESE.contains(&trimmed) {
        return true;
    }
    // DSH also renders "N 个子智能体运行中" in the row while sub-agents work.
    let digit_prefix = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digit_prefix > 0 && trimmed[digit_prefix..].trim_start().starts_with("个子智能体") {
        return true;
    }
    matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "running" | "completed" | "failed" | "waiting" | "idle" | "in progress"
    )
}

/// Parse a relative-time label into `(value_secs, granularity_secs)`. DSH shows
/// `刚刚` / `N分钟` / `N小时` / `N天`; compact English forms are accepted too.
/// `None` for anything else (titles that merely contain digits stay titles).
fn parse_relative_time_label(text: &str) -> Option<(u64, u64)> {
    let trimmed = text.trim();
    if trimmed == "刚刚" || trimmed.eq_ignore_ascii_case("just now") {
        return Some((0, 60));
    }
    let without_suffix = trimmed.trim_end_matches('前').trim();
    let digit_start = without_suffix.find(|ch: char| ch.is_ascii_digit())?;
    let digits: String = without_suffix[digit_start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let value = digits.parse::<u64>().ok()?;
    let unit_text = without_suffix[digit_start + digits.len()..].trim();
    let unit = if unit_text == "秒" || unit_text == "s" || unit_text.starts_with("second") {
        1
    } else if unit_text == "分钟"
        || unit_text == "分"
        || unit_text == "m"
        || unit_text.starts_with("min")
    {
        60
    } else if unit_text == "小时"
        || unit_text == "时"
        || unit_text == "h"
        || unit_text.starts_with("hour")
    {
        3600
    } else if unit_text == "天" || unit_text == "d" || unit_text.starts_with("day") {
        86400
    } else {
        return None;
    };
    Some((value.saturating_mul(unit), unit))
}

/// Group AX rows into workspace → session rows. DSH renders a workspace row
/// with exactly one text; every following multi-text row is one of its sessions.
fn parse_sidebar_sessions(rows: &[crate::ax::Row]) -> Vec<SidebarSession> {
    let mut sessions = Vec::new();
    let mut workspace: Option<String> = None;
    for row in rows {
        let texts: Vec<&crate::ax::TextRun> = row
            .texts
            .iter()
            .filter(|run| !run.text.trim().is_empty())
            .collect();
        if texts.is_empty() {
            continue;
        }
        if texts.len() == 1 {
            // A one-text row is a workspace/project row.
            workspace = Some(texts[0].text.trim().to_string());
            continue;
        }
        let mut title_runs: Vec<&crate::ax::TextRun> = Vec::new();
        let mut status = None;
        let mut time = None;
        for run in &texts {
            let text = run.text.trim();
            if let Some(parsed) = parse_relative_time_label(text) {
                time.get_or_insert(parsed);
                continue;
            }
            if is_status_label(text) {
                status.get_or_insert_with(|| text.to_string());
                continue;
            }
            title_runs.push(run);
        }
        // The longest remaining text is the title; status/time labels are short.
        let Some(title_run) = title_runs
            .into_iter()
            .max_by_key(|run| run.text.trim().chars().count())
        else {
            continue;
        };
        sessions.push(SidebarSession {
            workspace: workspace.clone(),
            title: title_run.text.trim().to_string(),
            title_frame: title_run.frame,
            row_frame: row.frame,
            status,
            time_secs_ago: time.map(|(value, _)| value),
            time_unit_secs: time.map(|(_, unit)| unit),
        });
    }
    sessions
}

/// Normalised title key: lowercase, collapsed whitespace, DSH's leading markdown
/// `#`, and a trailing ellipsis from truncated labels removed.
fn normalized_title_key(title: &str) -> String {
    let mut text = normalize_title(title);
    while let Some(rest) = text.strip_prefix('#') {
        text = rest.trim_start().to_string();
    }
    text.trim_end_matches('…')
        .trim_end_matches("...")
        .trim()
        .to_string()
}

/// `100` exact normalised match, `60` for a prefix match in either direction
/// (tolerates DSH's truncated row labels), `0` otherwise. Prefix matches need
/// at least [`MIN_TITLE_PREFIX_CHARS`] shared characters.
fn title_match_score(stored: &str, row_title: &str) -> u32 {
    let stored_key = normalized_title_key(stored);
    let row_key = normalized_title_key(row_title);
    if stored_key.is_empty() || row_key.is_empty() {
        return 0;
    }
    if stored_key == row_key {
        return 100;
    }
    let (short, long) = if stored_key.chars().count() <= row_key.chars().count() {
        (stored_key.as_str(), row_key.as_str())
    } else {
        (row_key.as_str(), stored_key.as_str())
    };
    if short.chars().count() < MIN_TITLE_PREFIX_CHARS {
        return 0;
    }
    if long.starts_with(short) {
        60
    } else {
        0
    }
}

/// Last path component of a workspace path (project name for matching).
fn workspace_basename(path: &str) -> Option<String> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
}

fn project_match_score(expected_basename: &str, row_workspace: &str) -> u32 {
    let expected = normalize_title(expected_basename);
    let actual = normalize_title(row_workspace);
    if expected.is_empty() || actual.is_empty() {
        return 0;
    }
    if expected == actual {
        20
    } else if actual.contains(&expected) || expected.contains(&actual) {
        10
    } else {
        0
    }
}

/// Relative-time evidence: the label's unit is the rounding tolerance. DSH
/// rounds down, so `N分钟` covers `[N-1, N+2)` minutes.
fn time_match_score(expected_age_secs: u64, label_secs: u64, unit_secs: u64) -> u32 {
    if unit_secs == 0 {
        return 0;
    }
    let lower = label_secs.saturating_sub(unit_secs);
    let upper = label_secs.saturating_add(unit_secs.saturating_mul(2));
    if expected_age_secs >= lower && expected_age_secs <= upper {
        15
    } else {
        0
    }
}

/// Score a row on all three signals. A title match is required (a workspace or
/// time match alone must never click a guessed row); project and time only
/// disambiguate between title matches.
fn session_candidate_score(session: &SidebarSession, task: &TaskRecord, now_secs: u64) -> u32 {
    let stored = task
        .target
        .window_title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&task.title);
    let title = title_match_score(stored, &session.title);
    if title == 0 {
        return 0;
    }
    let project = match (
        task.target
            .workspace_path
            .as_deref()
            .and_then(workspace_basename),
        session.workspace.as_deref(),
    ) {
        (Some(expected), Some(actual)) => project_match_score(&expected, actual),
        _ => 0,
    };
    let time = match (session.time_secs_ago, session.time_unit_secs) {
        (Some(label_secs), Some(unit)) => time_match_score(
            now_secs.saturating_sub(task.last_activity_secs),
            label_secs,
            unit,
        ),
        _ => 0,
    };
    title + project + time
}

fn select_session<'a>(
    sessions: &'a [SidebarSession],
    task: &TaskRecord,
    now_secs: u64,
) -> SessionSelection<'a> {
    let mut best: Option<(u32, &'a SidebarSession)> = None;
    let mut tied = false;
    for session in sessions {
        let score = session_candidate_score(session, task, now_secs);
        if score == 0 {
            continue;
        }
        match best {
            None => {
                best = Some((score, session));
                tied = false;
            }
            Some((best_score, _)) => {
                if score > best_score {
                    best = Some((score, session));
                    tied = false;
                } else if score == best_score {
                    tied = true;
                }
            }
        }
    }
    match best {
        None => SessionSelection::None,
        Some(_) if tied => SessionSelection::Ambiguous,
        Some((_, session)) => SessionSelection::One(session),
    }
}

/// Strip the ` — <App>` window-title suffix that macOS appends.
fn strip_window_app_suffix<'a>(title: &'a str, app_name: &str) -> &'a str {
    let trimmed = title.trim();
    for separator in [" — ", " – ", " - "] {
        if let Some(stripped) = trimmed.strip_suffix(app_name) {
            if let Some(prefix) = stripped.strip_suffix(separator) {
                return prefix.trim();
            }
        }
    }
    trimmed
}

/// Does the focused window title (e.g. "多窗口… — DeepSeek Harness") match the
/// stored session title?
fn session_title_matches(window_title: &str, app_name: &str, stored_title: &str) -> bool {
    let focused = strip_window_app_suffix(window_title, app_name);
    title_match_score(stored_title, focused) > 0
}

/// Does a VSCode window title ("<file> — <folder>") name the workspace folder?
fn window_title_contains_workspace(window_title: &str, workspace_basename: &str) -> bool {
    let title = normalize_title(window_title);
    let basename = normalize_title(workspace_basename);
    !title.is_empty() && !basename.is_empty() && title.contains(&basename)
}

/// Find `code` in PATH first, then the given install directories.
fn resolve_code_cli(path_env: Option<&str>, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path_env) = path_env {
        for dir in std::env::split_paths(path_env).filter(|dir| !dir.as_os_str().is_empty()) {
            let candidate = dir.join("code");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    extra_dirs
        .iter()
        .map(|dir| dir.join("code"))
        .find(|candidate| candidate.is_file())
}

fn code_cli_extra_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]
}

/// Parse `<pid> <command>` lines from `ps -axo pid=,command=` and return the
/// first pid whose command contains `needle`.
fn pid_for_app_path(ps_output: &str, needle: &str) -> Option<u32> {
    ps_output.lines().find_map(|line| {
        let line = line.trim_start();
        let mut parts = line.splitn(2, char::is_whitespace);
        let pid: u32 = parts.next()?.parse().ok()?;
        let command = parts.next()?;
        command.contains(needle).then_some(pid)
    })
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

    #[test]
    fn session_reference_requires_the_exact_id() {
        assert!(reference_has_session("vscode-webview://chat/index.html?session=abc-123&view=1", "abc-123"));
        assert!(reference_has_session("/local/abc-123", "abc-123"));
        assert!(!reference_has_session("/local/abc-123-other", "abc-123"));
        assert!(!reference_has_session("/local/other", "abc-123"));
        assert!(!reference_has_session("/local/", ""));
    }

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
            completion: None,
            completion_id: None,
            status_reason: "task_started".into(),
            last_activity_secs: 0,
            collected_at_secs: 0,
            summary: None,
            model: None,
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
        let out = jump(&t, &[t.clone()]);
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
        let out = jump(&t, &[t.clone()]);
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
        // v0.6.0: sidebar AX session route — deep_link now means "session route".
        assert_eq!(spec("dsh").max_level, JumpLevel::L3);
        assert!(spec("dsh").deep_link);
        assert!(spec("dsh").notes.contains("AX"));

        assert_eq!(spec("zcode").bundle_id, "dev.zcode.app");
        assert_eq!(spec("zcode").app_name, "ZCode");
        assert!(!spec("zcode").deep_link);

        assert_eq!(spec("claude-code").bundle_id, "com.microsoft.VSCode");
        assert_eq!(spec("claude-code").app_name, "Visual Studio Code");
        assert_eq!(spec("claude-code").ax_process, "Code");
        assert_eq!(spec("claude-code").max_level, JumpLevel::L3);
        assert!(spec("claude-code").deep_link);
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

    // ── DSH sidebar session jump helpers ────────────────────────

    use crate::ax::{Rect, Row, TextRun};

    fn ax_row(frames: &[(&str, Option<Rect>)], frame: Option<Rect>) -> Row {
        Row {
            texts: frames
                .iter()
                .map(|(text, frame)| TextRun {
                    text: (*text).to_string(),
                    frame: *frame,
                })
                .collect(),
            frame,
        }
    }

    fn sample_rect(y: f64) -> Rect {
        Rect {
            x: 360.0,
            y,
            w: 140.0,
            h: 17.0,
        }
    }

    fn dsh_task(title: &str, workspace: &str, activity: u64, level: JumpLevel) -> TaskRecord {
        let mut record = task(level, JumpLevel::L3);
        record.id = "dsh:session-1".into();
        record.agent_key = "dsh".into();
        record.agent_name = "DSH".into();
        record.title = title.into();
        record.last_activity_secs = activity;
        record.target = JumpTarget {
            app_name: Some("DeepSeek Harness".into()),
            bundle_id: Some("com.deepseek.dsh".into()),
            deep_link: None,
            window_title: Some(title.into()),
            workspace_path: Some(workspace.into()),
            host_ref: Some(DSH_SIDEBAR_HOST_REF.into()),
            ..Default::default()
        };
        record.capability.max_level = JumpLevel::L3;
        record.level = level;
        record
    }

    #[test]
    fn has_session_route_accepts_deep_links_terminal_and_dsh_sidebar() {
        let plain = task(JumpLevel::L3, JumpLevel::L3);
        let mut plain = plain;
        plain.target.deep_link = None;
        plain.target.host_ref = None;
        assert!(!has_session_route(&plain));

        let mut deep_link = plain.clone();
        deep_link.target.deep_link = Some("codex://threads/x".into());
        assert!(has_session_route(&deep_link));

        let mut sidebar = plain.clone();
        sidebar.target.host_ref = Some(DSH_SIDEBAR_HOST_REF.into());
        assert!(has_session_route(&sidebar));

        let mut terminal = plain;
        terminal.target.terminal_host = Some("Terminal".into());
        assert!(has_session_route(&terminal));
    }

    #[test]
    fn session_title_matches_strips_the_app_window_suffix() {
        let stored = "多窗口询问提醒与跳转";
        assert!(session_title_matches(
            "多窗口询问提醒与跳转 — DeepSeek Harness",
            "DeepSeek Harness",
            stored
        ));
        // A running marker/prefix must not hide an exact title match.
        assert!(session_title_matches(
            "多窗口询问提醒与跳转 - DeepSeek Harness",
            "DeepSeek Harness",
            stored
        ));
        assert!(!session_title_matches(
            "另一个会话 — DeepSeek Harness",
            "DeepSeek Harness",
            stored
        ));
        // No suffix at all (e.g. a bare window title) still matches.
        assert!(session_title_matches(
            "多窗口询问提醒与跳转",
            "DeepSeek Harness",
            stored
        ));
    }

    #[test]
    fn title_match_score_prefers_exact_and_tolerates_truncation() {
        assert_eq!(title_match_score("Fix the parser", "Fix the parser"), 100);
        // Normalisation: case, dashes and whitespace collapse.
        assert_eq!(
            title_match_score("Fix  the Parser", "fix the parser — Codex"),
            60
        );
        // DSH truncates row labels: a prefix in either direction matches.
        assert_eq!(
            title_match_score(
                "给 agent-monitor 增加“设置页 + 3小时提醒功能",
                "# 给 agent-monitor 增加“设置页 +"
            ),
            60
        );
        assert_eq!(
            title_match_score("# 给 agent-monitor 增加“设置页 +", "给 agent-monitor 增加"),
            60
        );
        // Too short to be a safe prefix, unrelated text, or empty -> no match.
        assert_eq!(title_match_score("Fix", "Fix the parser"), 0);
        assert_eq!(title_match_score("Fix the parser", "Rewrite the parser"), 0);
        assert_eq!(title_match_score("", "Fix the parser"), 0);
        assert_eq!(title_match_score("Fix the parser", "  "), 0);
        // Trailing ellipsis from a truncated label is ignored.
        assert_eq!(title_match_score("多窗口询问提醒与跳转", "多窗口询问提醒…"), 60);
    }

    #[test]
    fn parse_relative_time_label_understands_dsh_labels() {
        assert_eq!(parse_relative_time_label("刚刚"), Some((0, 60)));
        assert_eq!(parse_relative_time_label("23分钟"), Some((23 * 60, 60)));
        assert_eq!(parse_relative_time_label("3 小时"), Some((3 * 3600, 3600)));
        assert_eq!(parse_relative_time_label("1天"), Some((86_400, 86_400)));
        assert_eq!(parse_relative_time_label("23分钟前"), Some((23 * 60, 60)));
        assert_eq!(parse_relative_time_label("just now"), Some((0, 60)));
        assert_eq!(parse_relative_time_label("5 min"), Some((300, 60)));
        // Titles that merely contain digits are not time labels.
        assert_eq!(parse_relative_time_label("# 给 agent-monitor 增加“设置页 +"), None);
        assert_eq!(parse_relative_time_label("2024 roadmap"), None);
        assert_eq!(parse_relative_time_label("13分15秒"), None);
        assert_eq!(parse_relative_time_label("进行中"), None);
    }

    #[test]
    fn status_labels_are_exact_not_substrings() {
        assert!(is_status_label("进行中"));
        assert!(is_status_label("已完成"));
        assert!(is_status_label("失败"));
        // DSH's sub-agent activity badge is a status, not a session title.
        assert!(is_status_label("1 个子智能体运行中"));
        assert!(is_status_label("12个子智能体运行中"));
        assert!(!is_status_label("进行中的重构"));
        assert!(!is_status_label("# 进行中与已完成：两个功能均已实现"));
        assert!(!is_status_label("子智能体协作调研"));
        assert!(is_status_label("running"));
    }

    #[test]
    fn sidebar_rows_group_workspaces_and_extract_session_titles() {
        // Mirrors the real DSH sidebar: one workspace row, then multi-text
        // session rows; a one-text session ("wo") is treated as a workspace row.
        let rows = vec![
            ax_row(&[("AgentMonitor", None)], Some(sample_rect(346.0))),
            ax_row(
                &[
                    ("进行中", None),
                    ("多窗口询问提醒与跳转", Some(sample_rect(389.0))),
                    ("23分钟", None),
                ],
                Some(sample_rect(382.0)),
            ),
            ax_row(
                &[("# 给 agent-monitor 增加“设置页 +", None), ("3小时", None)],
                Some(sample_rect(416.0)),
            ),
            ax_row(&[("wo", None)], Some(sample_rect(486.0))),
            ax_row(&[("审稿", None)], Some(sample_rect(722.0))),
        ];
        let sessions = parse_sidebar_sessions(&rows);
        assert_eq!(sessions.len(), 2, "only the two multi-text rows are sessions");

        let first = &sessions[0];
        assert_eq!(first.workspace.as_deref(), Some("AgentMonitor"));
        assert_eq!(first.title, "多窗口询问提醒与跳转");
        assert_eq!(first.status.as_deref(), Some("进行中"));
        assert_eq!(first.time_secs_ago, Some(23 * 60));
        assert_eq!(first.time_unit_secs, Some(60));
        assert_eq!(first.title_frame, Some(sample_rect(389.0)));
        assert_eq!(first.row_frame, Some(sample_rect(382.0)));

        let second = &sessions[1];
        assert_eq!(second.workspace.as_deref(), Some("AgentMonitor"));
        assert_eq!(second.title, "# 给 agent-monitor 增加“设置页 +");
        assert_eq!(second.status, None);
        assert_eq!(second.time_secs_ago, Some(3 * 3600));
        // The "wo" row reset the workspace, so "审稿" is the visible workspace
        // for rows after it; earlier sessions keep "AgentMonitor".
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn subagent_badge_does_not_become_the_session_title() {
        let rows = vec![
            ax_row(&[("AgentMonitor", None)], None),
            ax_row(
                &[
                    ("进行中", None),
                    ("1 个子智能体运行中", None),
                    ("多窗口询问提醒与跳转", Some(sample_rect(389.0))),
                    ("42分钟", None),
                ],
                Some(sample_rect(382.0)),
            ),
        ];
        let sessions = parse_sidebar_sessions(&rows);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "多窗口询问提醒与跳转");
        assert_eq!(sessions[0].status.as_deref(), Some("进行中"));
        assert_eq!(sessions[0].time_secs_ago, Some(42 * 60));
    }

    #[test]
    fn retry_after_a_missed_click_needs_a_fresh_unique_sane_row() {
        let sane = Rect {
            x: 265.0,
            y: 389.0,
            w: 140.0,
            h: 17.0,
        };
        let matched = FreshSelection::Match {
            title: "多窗口询问提醒与跳转".into(),
            frame: Some(sane),
        };
        assert_eq!(
            plan_retry(1, DSH_CLICK_ATTEMPTS, matched.clone()),
            RetryPlan::Retry {
                title: "多窗口询问提醒与跳转".into(),
                frame: sane,
            }
        );
        // Attempt budget exhausted: report honestly, never guess again.
        assert_eq!(
            plan_retry(2, DSH_CLICK_ATTEMPTS, matched.clone()),
            RetryPlan::GiveUp("session_click_unverified")
        );
        // A fresh ambiguous / missing selection is never clicked.
        assert_eq!(
            plan_retry(1, DSH_CLICK_ATTEMPTS, FreshSelection::Ambiguous),
            RetryPlan::GiveUp("session_ambiguous")
        );
        assert_eq!(
            plan_retry(1, DSH_CLICK_ATTEMPTS, FreshSelection::None),
            RetryPlan::GiveUp("session_row_not_found")
        );
        // A transient 1 px frame (layout in progress) must not be clicked.
        assert_eq!(
            plan_retry(
                1,
                DSH_CLICK_ATTEMPTS,
                FreshSelection::Match {
                    title: "t".into(),
                    frame: Some(Rect {
                        x: 265.0,
                        y: 346.0,
                        w: 140.0,
                        h: 1.0,
                    }),
                }
            ),
            RetryPlan::GiveUp("session_click_unverified")
        );
        assert_eq!(
            plan_retry(
                1,
                DSH_CLICK_ATTEMPTS,
                FreshSelection::Match {
                    title: "t".into(),
                    frame: None,
                }
            ),
            RetryPlan::GiveUp("session_click_unverified")
        );
    }

    // ── Permission repair helpers ───────────────────────────────

    #[test]
    fn tccutil_output_classification_distinguishes_reset_skipped_and_error() {
        // A successful reset (real output: "Successfully reset Accessibility").
        assert_eq!(
            classify_tccutil_result(true, "Successfully reset Accessibility\n"),
            TccReset::Reset
        );
        assert_eq!(classify_tccutil_result(true, ""), TccReset::Reset);
        // Real macOS output for a copy with no TCC record (exit 64).
        assert_eq!(
            classify_tccutil_result(
                false,
                "tccutil: No such bundle identifier \"com.agent.monitor\": \
                 The operation couldn’t be completed. (OSStatus error -10814.)\n"
            ),
            TccReset::Skipped
        );
        // Any other non-zero exit is a real error and keeps its message.
        match classify_tccutil_result(false, "operation not permitted") {
            TccReset::Error(detail) => assert!(detail.contains("operation not permitted")),
            other => panic!("expected an error, got {other:?}"),
        }
        match classify_tccutil_result(false, "") {
            TccReset::Error(detail) => assert!(!detail.is_empty()),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn reset_message_lists_services_and_explains_recovery() {
        let english = format_reset_message(&["Accessibility"], &["automation (AppleEvents)"], false);
        assert!(english.contains("Reset: Accessibility"));
        assert!(english.contains("automation (AppleEvents)"));
        assert!(english.contains("Reveal App"));
        assert!(english.contains("restart Agent Monitor"));
        // Scope is explicit: only this bundle's records were cleared.
        assert!(english.contains("Only records for com.agent.monitor were cleared"));

        let chinese = format_reset_message(&["辅助功能"], &["自动化（Apple 事件）"], true);
        assert!(chinese.contains("已重置：辅助功能"));
        assert!(chinese.contains("在访达中显示"));
        assert!(chinese.contains("重启 Agent Monitor"));
        assert!(chinese.contains("仅清除了 com.agent.monitor 的记录"));

        // Empty lists are explicit instead of producing a broken sentence.
        let empty = format_reset_message(&[], &[], false);
        assert!(empty.contains("Reset: none"));
        assert!(empty.contains("No record (nothing to reset): none"));
    }

    #[test]
    fn tccutil_argv_always_carries_the_bundle_id_last() {
        for service in ["Accessibility", "AppleEvents"] {
            let argv = tccutil_reset_argv(service, "com.agent.monitor").unwrap();
            // Exact shape — the bundle id must always be the final argument.
            assert_eq!(
                argv,
                vec![
                    "/usr/bin/tccutil".to_string(),
                    "reset".to_string(),
                    service.to_string(),
                    "com.agent.monitor".to_string(),
                ]
            );
            assert_eq!(argv.last().map(String::as_str), Some("com.agent.monitor"));
            assert_eq!(argv.len(), 4);
        }
        // Refused inputs can never produce a machine-wide `tccutil reset <service>`.
        assert!(tccutil_reset_argv("Accessibility", "").is_err());
        assert!(tccutil_reset_argv("Accessibility", "   ").is_err());
        assert!(tccutil_reset_argv("", "com.agent.monitor").is_err());
        assert_eq!(
            tccutil_reset_argv("Accessibility", "").unwrap_err(),
            "tccutil_bundle_id_required"
        );
    }

    #[test]
    fn poll_until_respects_its_timeout_and_returns_early() {
        // timeout 0: exactly one probe, then an honest timeout.
        let mut calls = 0;
        let none = poll_until(0, || {
            calls += 1;
            None::<u8>
        });
        assert_eq!(none, None);
        assert_eq!(calls, 1);

        // A later probe wins immediately (no full timeout wait).
        let mut calls = 0;
        let some = poll_until(1_000, || {
            calls += 1;
            (calls >= 2).then_some(7u8)
        });
        assert_eq!(some, Some(7));
        assert_eq!(calls, 2);
    }

    fn workspace(title: &str, expanded: bool, y: f64) -> SidebarWorkspace {
        SidebarWorkspace {
            title: title.into(),
            title_frame: Some(sample_rect(y)),
            row_frame: Some(sample_rect(y)),
            expanded,
        }
    }

    #[test]
    fn sidebar_workspace_rows_mark_group_expansion() {
        // Real DSH shape: workspace row, its sessions, then collapsed projects.
        let rows = vec![
            ax_row(
                &[("AgentMonitor", Some(sample_rect(354.0)))],
                Some(sample_rect(346.0)),
            ),
            ax_row(
                &[("进行中", None), ("多窗口询问提醒与跳转", None), ("23分钟", None)],
                Some(sample_rect(382.0)),
            ),
            ax_row(
                &[("审稿", Some(sample_rect(730.0)))],
                Some(sample_rect(722.0)),
            ),
            ax_row(
                &[("博士毕业论文", Some(sample_rect(768.0)))],
                Some(sample_rect(760.0)),
            ),
            ax_row(
                &[("DropDPO-Confidence", Some(sample_rect(806.0)))],
                Some(sample_rect(798.0)),
            ),
        ];
        let workspaces = parse_sidebar_workspaces(&rows);
        assert_eq!(workspaces.len(), 4);
        assert_eq!(workspaces[0].title, "AgentMonitor");
        assert!(workspaces[0].expanded, "sessions follow this workspace row");
        assert_eq!(workspaces[0].title_frame, Some(sample_rect(354.0)));
        assert!(!workspaces[1].expanded, "审稿 has no rendered sessions");
        assert!(!workspaces[2].expanded);
        assert!(!workspaces[3].expanded);
        // A session row alone never becomes a workspace.
        assert!(parse_sidebar_workspaces(&rows[1..2]).is_empty());
    }

    #[test]
    fn choose_workspace_prefers_exact_and_never_guesses() {
        let rows = vec![
            workspace("AgentMonitor", true, 346.0),
            workspace("审稿", false, 722.0),
            workspace("Paper-A", false, 760.0),
            workspace("Paper-B", false, 798.0),
        ];
        // Exact normalised match wins (and an expanded exact group is skipped).
        assert_eq!(choose_workspace(&rows, "审稿").map(|w| w.title.as_str()), Some("审稿"));
        // Unique "contains" fallback (project basename is a prefix).
        assert_eq!(
            choose_workspace(&[workspace("DropDPO-Confidence", false, 1.0)], "DropDPO")
                .map(|w| w.title.as_str()),
            Some("DropDPO-Confidence")
        );
        // Ambiguous contains -> no click.
        assert_eq!(choose_workspace(&rows, "Paper"), None);
        // Exact duplicate project names -> no click.
        let duplicates = vec![
            workspace("审稿", false, 1.0),
            workspace("审稿", false, 2.0),
        ];
        assert_eq!(choose_workspace(&duplicates, "审稿"), None);
        // No match / empty basename / already-expanded-only -> no click.
        assert_eq!(choose_workspace(&rows, "unrelated"), None);
        assert_eq!(choose_workspace(&rows, ""), None);
        let expanded = vec![workspace("AgentMonitor", true, 346.0)];
        assert_eq!(choose_workspace(&expanded, "AgentMonitor"), None);
    }

    #[test]
    fn workspace_basename_handles_trailing_slashes_and_empty_paths() {
        assert_eq!(
            workspace_basename("/Users/wei/Paper/DropDPO-Confidence/").as_deref(),
            Some("DropDPO-Confidence")
        );
        assert_eq!(workspace_basename("/tmp/ws").as_deref(), Some("ws"));
        assert_eq!(workspace_basename(""), None);
        assert_eq!(workspace_basename("   "), None);
        assert_eq!(workspace_basename("/"), None);
    }

    #[test]
    fn project_and_time_signals_disambiguate_identical_titles() {
        let now = 1_000_000;
        // Two sessions share the same (short) title but live in different
        // workspaces and have different activity ages.
        let rows = vec![
            ax_row(&[("AgentMonitor", None)], None),
            ax_row(
                &[("You are the frontend engineer", None), ("2小时", None)],
                Some(sample_rect(394.0)),
            ),
            ax_row(&[("OtherProject", None)], None),
            ax_row(
                &[("You are the frontend engineer", None), ("30分钟", None)],
                Some(sample_rect(428.0)),
            ),
        ];
        let sessions = parse_sidebar_sessions(&rows);
        // The task belongs to OtherProject and its activity matches that row's
        // "30分钟", so project + time both point at the second row.
        let task = dsh_task(
            "You are the frontend engineer",
            "/Users/wei/Documents/OtherProject",
            now - 30 * 60,
            JumpLevel::L3,
        );
        match select_session(&sessions, &task, now) {
            SessionSelection::One(session) => {
                assert_eq!(session.workspace.as_deref(), Some("OtherProject"));
                assert_eq!(session.time_secs_ago, Some(30 * 60));
            }
            other => panic!("expected a unique match, got {other:?}"),
        }
    }

    #[test]
    fn equal_scored_rows_are_ambiguous_and_never_clicked() {
        let now = 1_000_000;
        let rows = vec![
            ax_row(&[("AgentMonitor", None)], None),
            ax_row(
                &[("You are the Rust backend", None), ("5分钟", None)],
                Some(sample_rect(394.0)),
            ),
            ax_row(
                &[("You are the Rust backend", None), ("5分钟", None)],
                Some(sample_rect(428.0)),
            ),
        ];
        let sessions = parse_sidebar_sessions(&rows);
        let task = dsh_task(
            "You are the Rust backend",
            "/Users/wei/Documents/AgentMonitor",
            now - 5 * 60,
            JumpLevel::L3,
        );
        assert_eq!(
            select_session(&sessions, &task, now),
            SessionSelection::Ambiguous
        );
    }

    #[test]
    fn candidate_requires_a_title_match() {
        let now = 1_000_000;
        let rows = vec![
            ax_row(&[("AgentMonitor", None)], None),
            ax_row(
                &[("Unrelated session", None), ("5分钟", None)],
                Some(sample_rect(394.0)),
            ),
        ];
        let sessions = parse_sidebar_sessions(&rows);
        // Same workspace + matching time must not select a different title.
        let task = dsh_task(
            "You are the Rust backend",
            "/Users/wei/Documents/AgentMonitor",
            now - 5 * 60,
            JumpLevel::L3,
        );
        assert_eq!(select_session(&sessions, &task, now), SessionSelection::None);
    }

    #[test]
    fn time_match_score_tolerates_downward_rounding() {
        // "23分钟" means 23-24 minutes elapsed; label/unit = (1380, 60).
        assert_eq!(time_match_score(23 * 60, 23 * 60, 60), 15);
        assert_eq!(time_match_score(23 * 60 - 30, 23 * 60, 60), 15);
        assert_eq!(time_match_score(24 * 60 + 30, 23 * 60, 60), 15);
        assert_eq!(time_match_score(26 * 60, 23 * 60, 60), 0);
        assert_eq!(time_match_score(20 * 60, 23 * 60, 60), 0);
        // Without a granularity there is no time signal.
        assert_eq!(time_match_score(1_000, 1_000, 0), 0);
    }

    #[test]
    fn vscode_reuse_window_verifies_by_workspace_basename() {
        assert!(window_title_contains_workspace(
            "arr_submission.pdf — DropDPO-Confidence",
            "DropDPO-Confidence"
        ));
        assert!(window_title_contains_workspace(
            "results.md — dropdpo-confidence",
            "DropDPO-Confidence"
        ));
        assert!(!window_title_contains_workspace(
            "arr_submission.pdf — OtherProject",
            "DropDPO-Confidence"
        ));
        assert!(!window_title_contains_workspace("", "DropDPO-Confidence"));
        assert!(!window_title_contains_workspace("anything", ""));
    }

    #[test]
    fn code_cli_resolution_prefers_path_then_install_dirs() {
        let base = std::env::temp_dir().join(format!(
            "am-jump-code-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let in_path = base.join("path-bin");
        let extra = base.join("homebrew-bin");
        std::fs::create_dir_all(&in_path).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let path_env = in_path.to_string_lossy().to_string();

        // Only the install dir has a `code` -> fallback used.
        std::fs::write(extra.join("code"), b"").unwrap();
        assert_eq!(
            resolve_code_cli(Some(&path_env), &[extra.clone()]),
            Some(extra.join("code"))
        );
        // PATH wins even if an install dir also has one.
        std::fs::write(in_path.join("code"), b"").unwrap();
        assert_eq!(
            resolve_code_cli(Some(&path_env), &[extra.clone()]),
            Some(in_path.join("code"))
        );
        // Nothing found.
        assert_eq!(resolve_code_cli(Some("/nonexistent"), &[]), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pid_for_app_path_parses_ps_output() {
        let ps = "  123 /Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness --flag\n  456 /usr/bin/other\n  789 /Applications/Visual Studio Code.app/Contents/MacOS/Code";
        assert_eq!(
            pid_for_app_path(ps, "DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness"),
            Some(123)
        );
        assert_eq!(
            pid_for_app_path(ps, "Visual Studio Code.app/Contents/MacOS/"),
            Some(789)
        );
        assert_eq!(pid_for_app_path(ps, "ZCode.app/Contents/MacOS/"), None);
    }
}
