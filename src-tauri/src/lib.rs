//! Agent Monitor — desktop-pet task monitor for multiple coding agents.
//!
//! Responsibilities are split into modules:
//!   * `agents/*`  — one adapter per real local data source
//!   * `monitor`   — collects adapters into one deduplicated snapshot
//!   * `jump`      — graded, verified jump execution (L3→L0)
//!   * `pet`       — pure geometry + placement persistence
//!   * `settings`  — user configuration: defaults, validation, atomic storage
//!   * this file   — Tauri window/tray/command plumbing only

pub mod agents;
pub mod ax;
pub mod icons;
pub mod jump;
pub mod model;
pub mod monitor;
pub mod pet;
pub mod settings;

use model::{
    now_millis, AgentDescriptor, Edge, JumpLevel, JumpOutcome, PermissionKind, PetMode, PetPrefs,
    PetState, Snapshot,
};
use monitor::{Monitor, PendingAlert};
use pet::{
    anchored_resize, constrain, expanded_height_logical, finish_drag_with, hide_tab_rect,
    nearest_edge,
    pick_screen, position_from_rect, restore_rect, state_view, unhide_rect, PetStore, Rect,
    ScreenRect, EXPANDED_WIDTH,
};
use settings::{Appearance, Language, Settings, SettingsGroup, SettingsStore};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow};

static ANIMATION_GENERATION: AtomicU64 = AtomicU64::new(0);
static DRAG_GENERATION: AtomicU64 = AtomicU64::new(0);

/// macOS menu-bar template icons: a three-bar "sessions" mark, plus a variant
/// with a notification dot when a task needs the user. 72 px sources are scaled
/// by the tray library to an 18 pt status item, which keeps them crisp @2x.
static TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
static TRAY_ICON_ALERT: &[u8] = include_bytes!("../icons/tray-alert.png");
static TRAY_ICON_SHOWS_ALERT: AtomicBool = AtomicBool::new(false);

// ── Runtime state ───────────────────────────────────────────────

struct Runtime {
    monitor: Monitor,
    prefs: PetPrefs,
    store: PetStore,
    settings: Settings,
    settings_store: SettingsStore,
    icons: icons::IconStore,
    /// Path of the corrupt settings file kept aside on load (diagnostics only).
    settings_corrupt_backup: Option<PathBuf>,
    state: PetState,
    /// Window moves caused by us must not be mistaken for user drags.
    suppress_until_ms: u64,
    mode_before_drag: PetMode,
    /// Authoritative window rectangle: macOS applies set_position
    /// asynchronously, so reading it back immediately after a move is stale.
    last_rect: Option<Rect>,
    /// Wakes the scheduler immediately (file watcher events + settings changes).
    wake_tx: Sender<()>,
}

pub struct AppState {
    inner: Mutex<Runtime>,
    /// Taken exactly once by the scheduler thread in `run`.
    wake_rx: Mutex<Option<Receiver<()>>>,
}

impl AppState {
    fn new() -> Self {
        let settings_store = SettingsStore::default_store();
        let load = settings_store.load();
        let settings = load.settings.clone();
        if load.recovered_from_corrupt {
            eprintln!(
                "settings: corrupt settings.json replaced by defaults (backup: {:?})",
                load.backup
            );
        }
        let store = PetStore::new(settings::pet_path());
        let prefs = store.load();
        let icon_store = icons::IconStore::default_store();
        let mut monitor = Monitor::new(&settings.enabled_agents);
        monitor.restore_dismissed(settings::dismissed_path());
        let state = PetState {
            hidden: prefs.hidden,
            ..Default::default()
        };
        let (wake_tx, wake_rx) = channel();
        Self {
            inner: Mutex::new(Runtime {
                monitor,
                prefs,
                store,
                settings,
                settings_store,
                icons: icon_store,
                settings_corrupt_backup: load.backup,
                state,
                suppress_until_ms: 0,
                mode_before_drag: PetMode::Collapsed,
                last_rect: None,
                wake_tx,
            }),
            wake_rx: Mutex::new(Some(wake_rx)),
        }
    }
}

fn settings_snapshot(app: &AppHandle) -> Settings {
    app.state::<AppState>().inner.lock().unwrap().settings.clone()
}

/// `language: system` is resolved once per call against `AppleLocale`.
fn resolved_language(settings: &Settings) -> Language {
    settings.language.resolve(system_is_chinese())
}

fn is_zh(settings: &Settings) -> bool {
    resolved_language(settings) == Language::Zh
}

/// Wake the scheduler so settings changes take effect immediately.
fn wake(app: &AppHandle) {
    let state = app.state::<AppState>();
    let tx = state.inner.lock().unwrap().wake_tx.clone();
    let _ = tx.send(());
}

// ── Screen + window helpers ─────────────────────────────────────

fn build_screens(win: &WebviewWindow) -> Vec<ScreenRect> {
    let mut out = Vec::new();
    if let Ok(monitors) = win.available_monitors() {
        for m in monitors {
            let p = m.position();
            let s = m.size();
            let wa = m.work_area();
            out.push(ScreenRect {
                key: m
                    .name()
                    .cloned()
                    .unwrap_or_else(|| format!("{}x{}+{}+{}", s.width, s.height, p.x, p.y)),
                frame: Rect::new(p.x as f64, p.y as f64, s.width as f64, s.height as f64),
                visible: Rect::new(
                    wa.position.x as f64,
                    wa.position.y as f64,
                    wa.size.width as f64,
                    wa.size.height as f64,
                ),
                scale: m.scale_factor(),
            });
        }
    }
    if out.is_empty() {
        out.push(ScreenRect {
            key: "primary".into(),
            frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            visible: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            scale: 1.0,
        });
    }
    out
}

fn window_rect(win: &WebviewWindow) -> Rect {
    let pos = win.outer_position().unwrap_or(PhysicalPosition::new(0, 0));
    let size = win.outer_size().unwrap_or(PhysicalSize::new(72, 72));
    Rect::new(
        pos.x as f64,
        pos.y as f64,
        size.width as f64,
        size.height as f64,
    )
}

fn set_rect(win: &WebviewWindow, r: Rect) {
    let _ = win.set_size(PhysicalSize::new(
        r.w.round().max(1.0) as u32,
        r.h.round().max(1.0) as u32,
    ));
    let _ = win.set_position(PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
}

fn screen_for(win: &WebviewWindow, rect: &Rect) -> (Vec<ScreenRect>, usize) {
    let screens = build_screens(win);
    let (cx, cy) = rect.center();
    let idx = pick_screen(&screens, cx, cy);
    (screens, idx)
}

fn tracked_rect(app: &AppHandle, win: &WebviewWindow) -> Rect {
    let state = app.state::<AppState>();
    let rt = state.inner.lock().unwrap();
    rt.last_rect.unwrap_or_else(|| window_rect(win))
}

fn remember_rect(app: &AppHandle, rect: Rect) {
    let state = app.state::<AppState>();
    let mut rt = state.inner.lock().unwrap();
    rt.last_rect = Some(rect);
}

fn target_size(
    mode: PetMode,
    rows: usize,
    collapsed_size: f64,
    sliver: f64,
    scale: f64,
    current: Rect,
) -> (f64, f64) {
    match mode {
        PetMode::Expanded => (
            EXPANDED_WIDTH * scale,
            expanded_height_logical(rows) * scale,
        ),
        // Edge-hidden is a small square tab (`hidden_sliver` per side), not the
        // full collapsed square: only that nub stays visible.
        PetMode::EdgeHidden => (sliver, sliver),
        PetMode::Collapsed | PetMode::TrayHidden => {
            (collapsed_size * scale, collapsed_size * scale)
        }
        PetMode::Dragging => (current.w, current.h),
    }
}

fn animate(win: &WebviewWindow, from: Rect, to: Rect, enabled: bool) {
    if !enabled {
        set_rect(win, to);
        return;
    }
    let generation = ANIMATION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if (from.x - to.x).abs() < 1.0
        && (from.y - to.y).abs() < 1.0
        && (from.w - to.w).abs() < 1.0
        && (from.h - to.h).abs() < 1.0
    {
        set_rect(win, to);
        return;
    }
    let win = win.clone();
    std::thread::spawn(move || {
        const STEPS: u32 = 7;
        for i in 1..=STEPS {
            if ANIMATION_GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            let t = i as f64 / STEPS as f64;
            let e = 1.0 - (1.0 - t).powi(3);
            set_rect(
                &win,
                Rect::new(
                    from.x + (to.x - from.x) * e,
                    from.y + (to.y - from.y) * e,
                    from.w + (to.w - from.w) * e,
                    from.h + (to.h - from.h) * e,
                ),
            );
            std::thread::sleep(Duration::from_millis(18));
        }
    });
}

fn persist(app: &AppHandle, rect: Rect, edge: Edge, mode: PetMode) {
    let state = app.state::<AppState>();
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let (screens, idx) = screen_for(&win, &rect);
    let mut rt = state.inner.lock().unwrap();
    rt.prefs.position = position_from_rect(&rect, &screens[idx], edge);
    rt.prefs.hidden = mode.is_offscreen();
    rt.state.edge = edge;
    rt.state.hidden = mode.is_offscreen();
    rt.store.save(&rt.prefs);
}

/// Apply a display mode to the real window, anchored to the current edge.
fn enter_mode(app: &AppHandle, mode: PetMode, animated: bool) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let current = tracked_rect(app, &win);
    let (screens, idx) = screen_for(&win, &current);
    let scale = screens[idx].scale;
    let visible = screens[idx].visible;
    let state = app.state::<AppState>();
    let (rows, edge, was_mode, settings) = {
        let rt = state.inner.lock().unwrap();
        (
            rt.settings.visible_rows(rt.monitor.snapshot().tasks.len()),
            rt.state.edge,
            rt.state.mode,
            rt.settings.clone(),
        )
    };
    let collapsed_size = f64::from(settings.collapsed_size);
    let edge = if edge == Edge::None {
        nearest_edge(&current, &visible)
    } else {
        edge
    };
    let sliver = f64::from(settings.hidden_sliver) * scale;
    let (tw, th) = target_size(mode, rows, collapsed_size, sliver, scale, current);
    let base = Rect::new(current.x, current.y, tw, th);
    let target = match mode {
        // The tab keeps the pet's position along the edge, so expanding again
        // grows from where the user left it.
        PetMode::EdgeHidden => hide_tab_rect(current, &visible, edge, sliver),
        PetMode::Expanded => {
            if was_mode.is_offscreen() {
                unhide_rect(current, edge, (tw, th), &visible)
            } else {
                anchored_resize(base, edge, (tw, th), &visible)
            }
        }
        PetMode::Collapsed => {
            let base = if was_mode.is_offscreen() {
                unhide_rect(current, edge, (tw, th), &visible)
            } else {
                current
            };
            anchored_resize(base, edge, (tw, th), &visible)
        }
        PetMode::Dragging => current,
        // Tray-hidden windows keep their last on-screen rect; placing them again
        // would move a hidden window for no reason.
        PetMode::TrayHidden => current,
    };

    {
        let mut rt = state.inner.lock().unwrap();
        rt.state.mode = mode;
        rt.state.edge = edge;
        rt.state.hidden = mode.is_offscreen();
        rt.suppress_until_ms = now_millis() + 420;
        let (screens, idx) = screen_for(&win, &target);
        rt.prefs.position = position_from_rect(&target, &screens[idx], edge);
        rt.prefs.hidden = rt.state.hidden;
        let view = state_view(&rt.state, &rt.settings);
        rt.store.save(&rt.prefs);
        drop(rt);
        let _ = app.emit("pet-state", view);
    }

    remember_rect(app, target);
    if std::env::var("AGENT_MONITOR_DEBUG_GEOMETRY").is_ok() {
        eprintln!(
            "geometry: mode={:?} current={:?} target={:?} rows={} scale={} sliver={}",
            mode, current, target, rows, scale, settings.hidden_sliver
        );
    }
    if mode == PetMode::TrayHidden {
        let _ = win.hide();
        return;
    }
    let animate_window = match settings.animations {
        settings::Animations::Off => false,
        settings::Animations::Always => true,
        // `system` is the historical behaviour; WebKit's prefers-reduced-motion
        // is still honoured for CSS transitions in the page.
        settings::Animations::System => true,
    };
    if animated && animate_window {
        animate(&win, current, target, animate_window);
    } else {
        set_rect(&win, target);
    }
    persist(app, target, edge, mode);
}

/// Keep the pet inside a connected screen after display changes / unplug.
fn reconcile_screens(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let current = tracked_rect(app, &win);
    let (screens, idx) = screen_for(&win, &current);
    if screens.is_empty() {
        return;
    }
    let visible = screens[idx].visible;
    let (mode, edge, sliver) = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        (
            rt.state.mode,
            rt.state.edge,
            f64::from(rt.settings.hidden_sliver),
        )
    };
    if mode == PetMode::Dragging || mode == PetMode::TrayHidden {
        return;
    }
    let target = if mode == PetMode::EdgeHidden && edge != Edge::None {
        hide_tab_rect(current, &visible, edge, sliver * screens[idx].scale)
    } else {
        constrain(current, &visible)
    };
    if (target.x - current.x).abs() > 0.5
        || (target.y - current.y).abs() > 0.5
        || (target.w - current.w).abs() > 0.5
        || (target.h - current.h).abs() > 0.5
    {
        app.state::<AppState>().inner.lock().unwrap().suppress_until_ms = now_millis() + 300;
        remember_rect(app, target);
        set_rect(&win, target);
        persist(app, target, edge, mode);
    }
}

fn show_pet(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let mode = { app.state::<AppState>().inner.lock().unwrap().state.mode };
        let _ = win.show();
        if mode.is_offscreen() || mode == PetMode::Collapsed {
            enter_mode(app, PetMode::Collapsed, false);
        }
    }
}

fn hide_pet_to_edge(app: &AppHandle) {
    enter_mode(app, PetMode::EdgeHidden, true);
}

/// `edge_hide_enabled=false`: hiding orders the window out (no off-screen
/// geometry); the tray menu / second launch bring it back.
fn hide_pet_to_tray(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    {
        let state = app.state::<AppState>();
        let mut rt = state.inner.lock().unwrap();
        rt.state.mode = PetMode::TrayHidden;
        rt.state.hidden = true;
        rt.prefs.hidden = true;
        rt.store.save(&rt.prefs);
        let view = state_view(&rt.state, &rt.settings);
        drop(rt);
        let _ = app.emit("pet-state", view);
    }
    let _ = win.hide();
}

fn hide_pet_now(app: &AppHandle) {
    if settings_snapshot(app).edge_hide_enabled {
        hide_pet_to_edge(app);
    } else {
        hide_pet_to_tray(app);
    }
}

fn reset_pet_position(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let screens = build_screens(&win);
    let idx = pick_screen(&screens, 0.0, 0.0);
    let s = &screens[idx];
    let collapsed = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        f64::from(rt.settings.collapsed_size)
    };
    let size = collapsed * s.scale;
    let rect = constrain(
        Rect::new(
            s.visible.right() - size - 24.0 * s.scale,
            s.visible.y + s.visible.h * 0.45,
            size,
            size,
        ),
        &s.visible,
    );
    {
        let state = app.state::<AppState>();
        let mut rt = state.inner.lock().unwrap();
        rt.state.edge = Edge::Right;
        rt.state.mode = PetMode::Collapsed;
        rt.state.hidden = false;
        rt.prefs.hidden = false;
        rt.suppress_until_ms = now_millis() + 300;
        rt.store.save(&rt.prefs);
    }
    let _ = win.show();
    remember_rect(app, rect);
    set_rect(&win, rect);
    persist(app, rect, Edge::Right, PetMode::Collapsed);
}

// ── Settings application ────────────────────────────────────────

fn settings_window_title(settings: &Settings) -> String {
    if is_zh(settings) {
        "设置".into()
    } else {
        "Settings".into()
    }
}

/// `set_theme(Some(Light|Dark))` / `None` (follows the system). On macOS the
/// theme is app-wide, so applying it to the pet window is enough; the settings
/// window is included for clarity and future platforms.
fn apply_theme(app: &AppHandle, appearance: Appearance) {
    let theme = match appearance {
        Appearance::System => None,
        Appearance::Light => Some(tauri::Theme::Light),
        Appearance::Dark => Some(tauri::Theme::Dark),
    };
    for label in ["main", "settings"] {
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.set_theme(theme);
        }
    }
}

/// Apply a settings change to the running app. Never restarts anything: the
/// scheduler is woken, the registry rebuilt only when the agent switches change,
/// and both webviews receive one event with the new values.
fn apply_settings(app: &AppHandle, previous: &Settings) {
    let (current, mode) = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        (rt.settings.clone(), rt.state.mode)
    };

    apply_theme(app, current.appearance);

    if previous.enabled_agents != current.enabled_agents {
        let changed = {
            let state = app.state::<AppState>();
            let mut rt = state.inner.lock().unwrap();
            rt.monitor.set_enabled_agents(&current.enabled_agents)
        };
        if changed {
            let snapshot = {
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                let retention = rt.settings.retention_secs();
                rt.monitor.scan(true, retention);
                rt.monitor.snapshot().clone()
            };
            let _ = app.emit("tasks-updated", &snapshot);
            // A registry rebuild re-seeds the question detector, so this is
            // normally empty; draining keeps the invariant "scan → drain".
            drain_attention(app);
        }
    }

    if previous.language != current.language || previous.edge_hide_enabled != current.edge_hide_enabled
    {
        update_tray_menu(app);
        if let Some(win) = app.get_webview_window("settings") {
            let _ = win.set_title(&settings_window_title(&current));
        }
    }

    if mode == PetMode::EdgeHidden && previous.edge_hide_enabled && !current.edge_hide_enabled {
        // The feature that parked the window off-screen was removed: keep the
        // window reachable (tray-hidden) instead of leaving an unreachable sliver.
        hide_pet_to_tray(app);
    } else if mode == PetMode::Expanded || mode == PetMode::EdgeHidden {
        let geometry_changed = previous.max_visible_rows != current.max_visible_rows
            || previous.hidden_sliver != current.hidden_sliver
            || previous.edge_hide_enabled != current.edge_hide_enabled;
        if geometry_changed && !(mode == PetMode::EdgeHidden && !current.edge_hide_enabled) {
            enter_mode(app, mode, false);
        }
    }

    wake(app);
    let _ = app.emit("settings-changed", &current);
    let view = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        state_view(&rt.state, &rt.settings)
    };
    let _ = app.emit("pet-state", view);
    let snapshot = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.monitor.snapshot().clone()
    };
    update_tray_tooltip(app, &snapshot);
}

/// Normalise, persist, then apply. Returns the normalized settings so the
/// caller (settings window) can render exactly what was stored.
fn commit_settings(app: &AppHandle, next: Settings) -> Settings {
    let (normalized, previous, saved) = {
        let state = app.state::<AppState>();
        let mut rt = state.inner.lock().unwrap();
        let previous = rt.settings.clone();
        let mut normalized = next;
        normalized.normalize();
        rt.settings = normalized.clone();
        let saved = rt.settings_store.save(&normalized);
        (normalized, previous, saved)
    };
    if !saved {
        eprintln!("settings: failed to persist settings.json");
    }
    apply_settings(app, &previous);
    normalized
}

// ── Settings window ─────────────────────────────────────────────

/// Create the settings window on first use (lazy). Keeping it out of the launch
/// path means a user who never opens Settings pays no extra webview/RSS at idle;
/// the first open absorbs webview startup like a normal macOS Preferences pane.
fn ensure_settings_window(app: &AppHandle, settings: &Settings) -> Option<tauri::WebviewWindow> {
    if let Some(win) = app.get_webview_window("settings") {
        return Some(win);
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title(settings_window_title(settings))
    .inner_size(620.0, 560.0)
    .min_inner_size(560.0, 440.0)
    .resizable(false)
    .maximizable(false)
    .minimizable(true)
    .decorations(true)
    .visible(false)
    .center()
    .build()
    .ok()?;

    let theme = match settings.appearance {
        Appearance::System => None,
        Appearance::Light => Some(tauri::Theme::Light),
        Appearance::Dark => Some(tauri::Theme::Dark),
    };
    let _ = win.set_theme(theme);

    // macOS Settings behaviour: closing hides the window (state and scroll
    // position are preserved); the app keeps running in the menu bar.
    let handle = app.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(win) = handle.get_webview_window("settings") {
                let _ = win.hide();
            }
            #[cfg(target_os = "macos")]
            let _ = handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
        }
    });
    Some(win)
}

/// Show/focus the single settings window. Opening Settings is an explicit user
/// action, so focusing it is correct; the app stays accessory (no Dock icon).
fn show_settings_window(app: &AppHandle) {
    let settings = settings_snapshot(app);
    let Some(win) = ensure_settings_window(app, &settings) else {
        return;
    };
    {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        #[cfg(target_os = "macos")]
        {
            let handle = app.clone();
            std::thread::spawn(move || {
                for delay in [90u64, 420] {
                    std::thread::sleep(Duration::from_millis(delay));
                    let _ = handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            });
        }
    }
}

fn hide_settings_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.hide();
    }
}

// ── Commands ────────────────────────────────────────────────────

#[tauri::command]
fn ping() -> String {
    "pong".into()
}

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Snapshot {
    state.inner.lock().unwrap().monitor.snapshot().clone()
}

#[tauri::command]
fn get_agents(state: State<'_, AppState>) -> Vec<AgentDescriptor> {
    state.inner.lock().unwrap().monitor.descriptors()
}

#[tauri::command]
fn refresh(app: AppHandle, state: State<'_, AppState>) -> Snapshot {
    let snapshot = {
        let mut rt = state.inner.lock().unwrap();
        let retention = rt.settings.retention_secs();
        rt.monitor.scan(true, retention);
        rt.monitor.snapshot().clone()
    };
    let _ = app.emit("tasks-updated", &snapshot);
    update_tray_tooltip(&app, &snapshot);
    drain_attention(&app);
    snapshot
}

#[tauri::command]
fn dismiss_task(app: AppHandle, state: State<'_, AppState>, task_id: String) -> bool {
    let ok = {
        let mut rt = state.inner.lock().unwrap();
        rt.monitor.dismiss(&task_id)
    };
    if ok {
        let snapshot = state.inner.lock().unwrap().monitor.snapshot().clone();
        let _ = app.emit("tasks-updated", &snapshot);
        update_tray_tooltip(&app, &snapshot);
    }
    ok
}

#[tauri::command]
fn restore_dismissed_task(app: AppHandle, state: State<'_, AppState>, task_id: String) -> bool {
    let ok = { state.inner.lock().unwrap().monitor.undismiss(&task_id) };
    if ok {
        let snapshot = {
            let mut rt = state.inner.lock().unwrap();
            let retention = rt.settings.retention_secs();
            rt.monitor.scan(true, retention);
            rt.monitor.snapshot().clone()
        };
        let _ = app.emit("tasks-updated", &snapshot);
        update_tray_tooltip(&app, &snapshot);
    }
    ok
}

#[tauri::command]
fn clear_dismissed(app: AppHandle, state: State<'_, AppState>) -> usize {
    let (removed, snapshot) = {
        let mut rt = state.inner.lock().unwrap();
        let removed = rt.monitor.clear_dismissed();
        let retention = rt.settings.retention_secs();
        rt.monitor.scan(true, retention);
        (removed, rt.monitor.snapshot().clone())
    };
    let _ = app.emit("tasks-updated", &snapshot);
    update_tray_tooltip(&app, &snapshot);
    removed
}

#[tauri::command]
async fn jump_to_task(app: AppHandle, task_id: String) -> Result<JumpOutcome, String> {
    let task = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.monitor.task(&task_id).cloned()
    };
    let outcome = match task {
        Some(task) => jump::jump(&task),
        None => JumpOutcome::failed(&task_id, JumpLevel::L0, "task_gone"),
    };
    let _ = app.emit("jump-feedback", &outcome);
    Ok(outcome)
}

#[tauri::command]
fn get_pet_state(state: State<'_, AppState>) -> pet::PetStateView {
    let rt = state.inner.lock().unwrap();
    state_view(&rt.state, &rt.settings)
}

/// Expand/collapse the pet. `focus` is only set for explicit clicks (never for
/// hover/automatic expansion) so the popover can lose key focus — and therefore
/// collapse — when the user clicks elsewhere, without stealing focus on hover.
#[tauri::command]
fn set_expanded(
    app: AppHandle,
    state: State<'_, AppState>,
    expanded: bool,
    focus: Option<bool>,
) {
    let mode = { state.inner.lock().unwrap().state.mode };
    if expanded {
        if mode != PetMode::Expanded {
            enter_mode(&app, PetMode::Expanded, true);
            if focus == Some(true) {
                if let Some(win) = app.get_webview_window("main") {
                    // Making the window key is what lets macOS deliver the
                    // outside-click blur that dismisses the popover.
                    let _ = win.set_focus();
                }
            }
        }
    } else if mode == PetMode::Expanded {
        enter_mode(&app, PetMode::Collapsed, true);
    }
}

/// Collapse the expanded popover. `force` bypasses the post-resize suppression
/// window and is used for explicit dismissals (click outside / Escape / blur);
/// the 800 ms mouse-leave path keeps respecting it to avoid self-collapse.
#[tauri::command]
fn request_collapse(app: AppHandle, state: State<'_, AppState>, force: Option<bool>) {
    let (mode, suppressed) = {
        let rt = state.inner.lock().unwrap();
        (rt.state.mode, now_millis() < rt.suppress_until_ms)
    };
    if mode == PetMode::Expanded && (force == Some(true) || !suppressed) {
        enter_mode(&app, PetMode::Collapsed, true);
    }
}

#[tauri::command]
fn hide_pet(app: AppHandle) {
    hide_pet_now(&app);
}

#[tauri::command]
fn show_pet_command(app: AppHandle) {
    show_pet(&app);
}

#[tauri::command]
fn reset_pet_position_command(app: AppHandle) {
    reset_pet_position(&app);
}

#[tauri::command]
fn open_permission_settings(kind: String) {
    let kind = if kind == "automation" {
        PermissionKind::Automation
    } else {
        PermissionKind::Accessibility
    };
    jump::open_permission_settings(kind);
}

/// Jump self-check (docs/JUMP.md): permission grants + per-agent capability
/// table. Blocking probes (`osascript`/`mdfind`/`ps`) run off the main thread;
/// the settings window calls this from its "Check again" button.
#[tauri::command]
async fn get_jump_diagnostics() -> jump::JumpDiagnostics {
    tauri::async_runtime::spawn_blocking(jump::collect_jump_diagnostics)
        .await
        .unwrap_or_else(|_| jump::JumpDiagnostics::unavailable())
}

/// Reveal the app bundle in Finder so it can be dragged into the Accessibility
/// list. Never opens System Settings by itself.
#[tauri::command]
fn reveal_app_in_finder() -> Result<(), String> {
    jump::reveal_app_in_finder()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.inner.lock().unwrap().settings.clone()
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: Settings) -> Settings {
    commit_settings(&app, settings)
}

#[tauri::command]
fn reset_settings(app: AppHandle, scope: String) -> Result<Settings, String> {
    let group = SettingsGroup::parse(&scope).ok_or_else(|| format!("unknown scope: {scope}"))?;
    let mut next = settings_snapshot(&app);
    next.reset_group(group);
    Ok(commit_settings(&app, next))
}

#[derive(serde::Serialize)]
struct SettingsMeta {
    data_dir: String,
    settings_path: String,
    pet_path: String,
    dismissed_path: String,
    version: u32,
    app_version: String,
    system_language: String,
    legacy_imported: bool,
    corrupt_backup: Option<String>,
}

#[tauri::command]
fn get_settings_meta(state: State<'_, AppState>) -> SettingsMeta {
    let rt = state.inner.lock().unwrap();
    SettingsMeta {
        data_dir: settings::data_dir().display().to_string(),
        settings_path: settings::settings_path().display().to_string(),
        pet_path: settings::pet_path().display().to_string(),
        dismissed_path: settings::dismissed_path().display().to_string(),
        version: rt.settings.version,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        system_language: if system_is_chinese() { "zh" } else { "en" }.into(),
        legacy_imported: rt.settings.legacy_imported,
        corrupt_backup: rt
            .settings_corrupt_backup
            .as_ref()
            .map(|p| p.display().to_string()),
    }
}

/// One-shot migration of the v0.3.x frontend `localStorage` values (`am-lang`,
/// `am-s.pollMs`). Other legacy fields are intentionally not migrated.
#[tauri::command]
fn import_legacy_settings(
    app: AppHandle,
    lang: Option<String>,
    poll_ms: Option<u64>,
) -> Option<Settings> {
    let already = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.settings.legacy_imported
    };
    if already {
        return None;
    }
    let mut next = settings_snapshot(&app);
    if let Some(lang) = lang {
        next.language = match lang.as_str() {
            "zh" => Language::Zh,
            "en" => Language::En,
            _ => next.language,
        };
    }
    if let Some(ms) = poll_ms {
        next.refresh_interval = match ms {
            3000 => settings::RefreshInterval::Sec3,
            5000 => settings::RefreshInterval::Sec5,
            10_000 => settings::RefreshInterval::Sec10,
            _ => next.refresh_interval,
        };
    }
    next.legacy_imported = true;
    Some(commit_settings(&app, next))
}

#[tauri::command]
fn get_custom_icons(state: State<'_, AppState>) -> icons::CustomIcons {
    state.inner.lock().unwrap().icons.load_all()
}

/// Store one icon slot (`collapsed` / `hidden`). The payload is validated in
/// Rust (magic bytes + 2 MB cap) before anything touches the disk.
#[tauri::command]
fn save_custom_icon(
    app: AppHandle,
    state: State<'_, AppState>,
    slot: String,
    data_url: String,
) -> Result<icons::IconMeta, String> {
    let meta = {
        let rt = state.inner.lock().unwrap();
        rt.icons.save_data_url(&slot, &data_url)?
    };
    broadcast_icons(&app);
    Ok(meta)
}

#[tauri::command]
fn clear_custom_icon(app: AppHandle, state: State<'_, AppState>, slot: String) -> Result<(), String> {
    {
        let rt = state.inner.lock().unwrap();
        rt.icons.clear(&slot)?;
    }
    broadcast_icons(&app);
    Ok(())
}

fn broadcast_icons(app: &AppHandle) {
    let payload = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.icons.load_all()
    };
    let _ = app.emit("icons-changed", payload);
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings_window(&app);
}

#[tauri::command]
fn close_settings(app: AppHandle) {
    hide_settings_window(&app);
}

#[tauri::command]
fn open_data_dir() -> Result<(), String> {
    std::process::Command::new("open")
        .arg(settings::data_dir())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open data directory: {e}"))
}

// ── Pending-question attention ──────────────────────────────────

/// Payload of the `attention-pending` event (see `docs/ATTENTION.md`).
#[derive(Clone, serde::Serialize)]
struct AttentionPayload {
    alerts: Vec<PendingAlert>,
    question_count: usize,
    auto_show: bool,
}

/// Localised notification title/body for one batch of new questions.
///
/// A single question names the agent and title; a batch collapses to a count so
/// the user gets exactly one banner (`docs/ATTENTION.md`).
fn attention_alert_text(alerts: &[PendingAlert], zh: bool) -> (String, String) {
    let title = if zh {
        "等待你的回答"
    } else {
        "Waiting for your answer"
    };
    let body = if alerts.len() == 1 {
        format!("{} · {}", alerts[0].agent_name, alerts[0].title)
    } else if zh {
        format!("{} 个会话在等你回答", alerts.len())
    } else {
        format!("{} sessions are waiting for you", alerts.len())
    };
    (title.to_string(), body.to_string())
}

/// Escape a Rust string for an AppleScript string literal. Quotes/backslashes
/// must not break out of the literal, and control characters (newlines from
/// task titles) would make the script invalid.
fn escape_osascript(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `osascript -e` script for one notification, with an optional "Glass" sound.
fn question_notification_script(title: &str, body: &str, sound: bool) -> String {
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_osascript(body),
        escape_osascript(title)
    );
    if sound {
        script.push_str(" sound name \"Glass\"");
    }
    script
}

/// Play the alert on a detached thread: a missing helper binary, a denied
/// notification banner or a slow `osascript` can never block a scan.
fn spawn_question_alert(alerts: Vec<PendingAlert>, notify: bool, sound: bool, zh: bool) {
    if alerts.is_empty() || (!notify && !sound) {
        return;
    }
    std::thread::spawn(move || {
        if notify {
            let (title, body) = attention_alert_text(&alerts, zh);
            let script = question_notification_script(&title, &body, sound);
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .output();
        } else if sound {
            // `alert_sound=true` with notifications off: an unsilenced alert is
            // still possible without a banner.
            let _ = std::process::Command::new("afplay")
                .arg("/System/Library/Sounds/Glass.aiff")
                .output();
        }
    });
}

/// `auto_show_on_question`: reveal a collapsed/edge-hidden/tray-hidden pet as an
/// expanded panel **without focusing it** (the user may be typing elsewhere).
/// An already expanded/dragging panel is left alone.
fn auto_reveal_for_questions(app: &AppHandle) {
    let mode = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.state.mode
    };
    if matches!(
        mode,
        PetMode::Collapsed | PetMode::EdgeHidden | PetMode::TrayHidden
    ) {
        show_pet(app);
        enter_mode(app, PetMode::Expanded, true);
    }
}

/// Drain the new questions from the last scan and surface them on every
/// channel: UI event, notification/sound, auto-reveal. Must be called *after*
/// the snapshot event so the UI can paint the new `question_count`.
fn drain_attention(app: &AppHandle) {
    let (alerts, question_count, auto_show, notify, sound, settings) = {
        let state = app.state::<AppState>();
        let mut rt = state.inner.lock().unwrap();
        let alerts = rt.monitor.take_new_questions();
        if alerts.is_empty() {
            return;
        }
        let settings = rt.settings.clone();
        let question_count = rt.monitor.snapshot().question_count;
        (
            alerts,
            question_count,
            settings.auto_show_on_question,
            settings.notify_on_question,
            settings.alert_sound,
            settings,
        )
    };
    // Language resolution may run `defaults` once: keep it outside the lock.
    let zh = is_zh(&settings);
    let payload = AttentionPayload {
        alerts: alerts.clone(),
        question_count,
        auto_show,
    };
    let _ = app.emit("attention-pending", &payload);
    if auto_show {
        auto_reveal_for_questions(app);
    }
    spawn_question_alert(alerts, notify, sound, zh);
}

/// Menu-bar tooltip text. Waiting questions get their own wording because
/// "N waiting for you" is more actionable than the generic attention count.
fn tray_tooltip_label(total: usize, waiting: usize, questions: usize, zh: bool) -> String {
    if total == 0 {
        return if zh {
            "Agent Monitor — 暂无会话".to_string()
        } else {
            "Agent Monitor — no sessions".to_string()
        };
    }
    if questions > 0 {
        let mut label = if zh {
            format!("Agent Monitor — {total} 个会话，{questions} 个待回答")
        } else {
            format!("Agent Monitor — {total} session(s), {questions} waiting for you")
        };
        // Keep the general attention count visible when it also covers failures.
        if waiting > questions {
            if zh {
                label.push_str(&format!("，{waiting} 个待处理"));
            } else {
                label.push_str(&format!(", {waiting} need attention"));
            }
        }
        return label;
    }
    if waiting > 0 {
        if zh {
            format!("Agent Monitor — {total} 个会话，{waiting} 个待处理")
        } else {
            format!("Agent Monitor — {total} session(s), {waiting} need attention")
        }
    } else if zh {
        format!("Agent Monitor — {total} 个会话")
    } else {
        format!("Agent Monitor — {total} session(s)")
    }
}

// ── Watcher + scheduler ─────────────────────────────────────────

fn watch_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let candidates = [
        home.join(".claude/projects"),
        home.join(".claude/file-history"),
        home.join(".claude/ide"),
        home.join(".codex"),
        home.join(".codex/sessions"),
        home.join(".dsh/storages/session_projcache/sessions"),
        home.join(".dsh/sessions"),
        home.join(".zcode/v2"),
    ];
    candidates.into_iter().filter(|p| p.exists()).collect()
}

fn start_watcher(tx: Sender<()>) {
    use notify::{RecursiveMode, Watcher};
    std::thread::spawn(move || {
        let (raw_tx, raw_rx) = channel::<()>();
        let mut watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = raw_tx.send(());
                }
            }) {
                Ok(w) => w,
                Err(_) => return,
            };
        let mut watched = 0;
        for root in watch_roots() {
            if watcher.watch(&root, RecursiveMode::Recursive).is_ok() {
                watched += 1;
            }
        }
        if watched == 0 {
            return;
        }
        loop {
            if raw_rx.recv().is_err() {
                return;
            }
            loop {
                match raw_rx.recv_timeout(Duration::from_millis(700)) {
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        }
    });
}

fn start_worker(app: AppHandle, rx: Receiver<()>) {
    std::thread::spawn(move || loop {
        let (interval, hidden, empty) = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            let hidden = rt.state.mode.is_offscreen() || rt.state.hidden;
            let empty = rt.monitor.snapshot().tasks.is_empty();
            let configured = rt.settings.active_refresh_secs();
            // Hidden or empty work stays on the 30 s floor regardless of the
            // configured active interval (battery/CPU contract).
            let interval = if hidden || empty {
                30
            } else {
                configured.unwrap_or(5)
            };
            (interval, hidden, empty)
        };
        let _ = rx.recv_timeout(Duration::from_secs(interval));
        while rx.try_recv().is_ok() {}

        let changed = {
            let state = app.state::<AppState>();
            let mut rt = state.inner.lock().unwrap();
            let retention = rt.settings.retention_secs();
            rt.monitor.scan(false, retention)
        };
        if changed {
            let snapshot = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.monitor.snapshot().clone()
            };
            // Emitted even while hidden: the collapsed pet / edge tab shows the
            // "new completions" indicator without expanding the panel. The UI
            // only re-renders the badge when collapsed, so this stays cheap.
            let _ = app.emit("tasks-updated", &snapshot);
            update_tray_tooltip(&app, &snapshot);
        }
        // New blocking questions are surfaced on every channel (UI event,
        // notification/sound, auto-reveal) — only when a scan produced some.
        drain_attention(&app);
        if !(hidden && empty) {
            reconcile_screens(&app);
        }
    });
}

fn system_is_chinese() -> bool {
    // Cached: this is consulted on every snapshot change by the tray updater.
    static ZH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ZH.get_or_init(|| {
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("zh")
            })
            .unwrap_or(false)
    })
}

fn update_tray_tooltip(app: &AppHandle, snapshot: &Snapshot) {
    // Single source of truth: the snapshot already applies the failure
    // freshness/cancellation rules (old or user-interrupted failures must not
    // keep the menu-bar alert dot alive) and counts blocking questions.
    let waiting = snapshot.attention_count;
    let questions = snapshot.question_count;
    let total = snapshot.tasks.len();
    let zh = is_zh(&settings_snapshot(app));
    let label = tray_tooltip_label(total, waiting, questions, zh);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(&label));
        let alert = waiting > 0;
        if TRAY_ICON_SHOWS_ALERT.swap(alert, Ordering::SeqCst) != alert {
            if std::env::var("AGENT_MONITOR_DEBUG_TRAY").is_ok() {
                eprintln!("tray: switched to {} icon", if alert { "alert" } else { "normal" });
            }
            let bytes = if alert { TRAY_ICON_ALERT } else { TRAY_ICON };
            if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
                // Atomic icon + template update avoids a visible flicker on macOS.
                let _ = tray.set_icon_with_as_template(Some(icon), true);
            }
        }
    }
}

// ── Tray ────────────────────────────────────────────────────────

fn tray_menu(
    app: &AppHandle,
    settings: &Settings,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    let zh = is_zh(settings);
    let settings_item =
        MenuItemBuilder::with_id("settings", if zh { "设置…" } else { "Settings…" }).build(app)?;
    let show_item = MenuItemBuilder::with_id("show", if zh { "显示桌宠" } else { "Show Pet" })
        .build(app)?;
    let hide_label = if settings.edge_hide_enabled {
        if zh {
            "隐藏到边缘"
        } else {
            "Hide to Edge"
        }
    } else if zh {
        "收起到托盘"
    } else {
        "Hide to Tray"
    };
    let hide_item = MenuItemBuilder::with_id("hide", hide_label).build(app)?;
    let reset_item =
        MenuItemBuilder::with_id("reset", if zh { "重置位置" } else { "Reset Position" })
            .build(app)?;
    let refresh_item =
        MenuItemBuilder::with_id("refresh", if zh { "刷新" } else { "Refresh" }).build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", if zh { "退出" } else { "Quit" }).build(app)?;
    MenuBuilder::new(app)
        .item(&settings_item)
        .separator()
        .item(&show_item)
        .item(&hide_item)
        .item(&reset_item)
        .separator()
        .item(&refresh_item)
        .separator()
        .item(&quit_item)
        .build()
}

/// Rebuild the tray menu after a language / hide-mode change.
fn update_tray_menu(app: &AppHandle) {
    let settings = settings_snapshot(app);
    if let Some(tray) = app.tray_by_id("main-tray") {
        if let Ok(menu) = tray_menu(app, &settings) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::tray::TrayIconEvent;

    let tray = app
        .tray_by_id("main-tray")
        .ok_or("tray icon missing; check tauri.conf.json")?;
    let settings = settings_snapshot(app);
    tray.set_menu(Some(tray_menu(app, &settings)?))?;
    tray.set_tooltip(Some("Agent Monitor"))?;
    // Left click belongs to the pet (show/collapse); the menu opens on right click.
    let _ = tray.set_show_menu_on_left_click(false);

    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click { button, .. } = event {
            if button == tauri::tray::MouseButton::Left {
                let app = tray.app_handle();
                let expanded = {
                    let state = app.state::<AppState>();
                    let mode = state.inner.lock().unwrap().state.mode;
                    mode == PetMode::Expanded
                };
                if expanded {
                    enter_mode(app, PetMode::Collapsed, true);
                } else {
                    show_pet(app);
                }
            }
        }
    });

    tray.on_menu_event(move |app, event| match event.id().as_ref() {
        "settings" => show_settings_window(app),
        "show" => show_pet(app),
        "hide" => hide_pet_now(app),
        "reset" => reset_pet_position(app),
        "refresh" => {
            let snapshot = {
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                let retention = rt.settings.retention_secs();
                rt.monitor.scan(true, retention);
                rt.monitor.snapshot().clone()
            };
            let _ = app.emit("tasks-updated", &snapshot);
            update_tray_tooltip(app, &snapshot);
            drain_attention(app);
        }
        "quit" => {
            {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.store.save(&rt.prefs);
                let _ = rt.settings_store.save(&rt.settings);
            }
            app.exit(0);
        }
        _ => {}
    });
    Ok(())
}

// ── Window events ───────────────────────────────────────────────

fn handle_window_moved(app: AppHandle) {
    let suppressed = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        now_millis() < rt.suppress_until_ms
    };
    if suppressed {
        return;
    }
    let generation = DRAG_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let state = app.state::<AppState>();
        let mut rt = state.inner.lock().unwrap();
        if rt.state.mode != PetMode::Dragging {
            rt.mode_before_drag = rt.state.mode;
            rt.state.mode = PetMode::Dragging;
            rt.state.hidden = false;
        }
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(240));
        if DRAG_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        let Some(win) = app.get_webview_window("main") else {
            return;
        };
        let current = window_rect(&win);
        let (screens, idx) = screen_for(&win, &current);
        let visible = screens[idx].visible;
        let scale = screens[idx].scale;
        let (snap_distance, mode_before, collapsed_size, sliver) = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            (
                f64::from(rt.settings.snap_distance),
                rt.mode_before_drag,
                f64::from(rt.settings.collapsed_size),
                f64::from(rt.settings.hidden_sliver) * scale,
            )
        };
        let (snapped, edge) = finish_drag_with(current, &visible, snap_distance * scale);
        let (mode, rows) = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            let mode = match mode_before {
                PetMode::Expanded => PetMode::Expanded,
                _ => PetMode::Collapsed,
            };
            (mode, rt.settings.visible_rows(rt.monitor.snapshot().tasks.len()))
        };
        let (tw, th) = target_size(mode, rows, collapsed_size, sliver, scale, snapped);
        let target = anchored_resize(
            Rect::new(snapped.x, snapped.y, tw, th),
            edge,
            (tw, th),
            &visible,
        );
        {
            let state = app.state::<AppState>();
            let mut rt = state.inner.lock().unwrap();
            rt.state.mode = mode;
            rt.state.edge = edge;
            rt.state.hidden = false;
            rt.suppress_until_ms = now_millis() + 350;
            let view = state_view(&rt.state, &rt.settings);
            drop(rt);
            let _ = app.emit("pet-state", view);
        }
        remember_rect(&app, target);
        set_rect(&win, target);
        persist(&app, target, edge, mode);
    });
}

// ── Headless verification entry ─────────────────────────────────

/// `agent-monitor --dump-tasks`: print the real snapshot as JSON without
/// starting the GUI. Honours `settings.json` (enabled agents + retention) so the
/// acceptance checklist can compare before/after exactly.
pub fn dump_tasks() {
    let settings = SettingsStore::default_store().load().settings;
    eprintln!(
        "settings: language={:?} retention={}h rows={} enabled=[codex={} dsh={} zcode={} claude_code={}]",
        settings.language,
        settings.retention_hours,
        settings.max_visible_rows,
        settings.enabled_agents.codex,
        settings.enabled_agents.dsh,
        settings.enabled_agents.zcode,
        settings.enabled_agents.claude_code
    );
    let mut monitor = Monitor::new(&settings.enabled_agents);
    monitor.scan(true, settings.retention_secs());
    let first: Vec<String> = monitor.snapshot().tasks.iter().map(|t| t.id.clone()).collect();
    monitor.scan(true, settings.retention_secs());
    let second: Vec<String> = monitor.snapshot().tasks.iter().map(|t| t.id.clone()).collect();
    eprintln!(
        "dedupe: first={} second={} stable={}",
        first.len(),
        second.len(),
        first == second
    );
    let snapshot = monitor.snapshot();
    match serde_json::to_string_pretty(snapshot) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("failed to serialise snapshot: {e}"),
    }
}

/// `agent-monitor --jump <task-id>`: run the real graded jump for one task and
/// print the outcome. Used by the Phase 2 acceptance evidence.
pub fn jump_task(task_id: &str) {
    let settings = SettingsStore::default_store().load().settings;
    let mut monitor = Monitor::new(&settings.enabled_agents);
    monitor.scan(true, settings.retention_secs());
    match monitor.task(task_id) {
        Some(task) => match serde_json::to_string_pretty(&jump::jump(task)) {
            Ok(json) => println!("{json}"),
            Err(e) => eprintln!("failed to serialise outcome: {e}"),
        },
        None => eprintln!("task not found in current snapshot: {task_id}"),
    }
}

/// `agent-monitor --dump-agents`: print adapter descriptors/capability matrix.
pub fn dump_agents() {
    let settings = SettingsStore::default_store().load().settings;
    let monitor = Monitor::new(&settings.enabled_agents);
    match serde_json::to_string_pretty(&monitor.descriptors()) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("failed to serialise agents: {e}"),
    }
}

/// `agent-monitor --jump-diagnostics`: print the same JSON as the
/// `get_jump_diagnostics` command without starting the GUI (support tickets).
pub fn dump_jump_diagnostics() {
    match serde_json::to_string_pretty(&jump::collect_jump_diagnostics()) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("failed to serialise jump diagnostics: {e}"),
    }
}

/// `agent-monitor --ax-rows <pid>`: read-only dump of the AX sidebar rows of a
/// process (texts + global screen frames) as JSON. Lets jump/DSH work be
/// verified without clicking anything.
pub fn dump_ax_rows(pid: u32) {
    #[derive(serde::Serialize)]
    struct AxRowsDump {
        pid: u32,
        trusted: bool,
        focused_window_title: Option<String>,
        window_titles: Vec<String>,
        rows: Vec<ax::Row>,
    }
    let dump = AxRowsDump {
        pid,
        trusted: ax::is_trusted(),
        focused_window_title: ax::focused_window_title(pid),
        window_titles: ax::window_titles(pid),
        rows: ax::rows(pid),
    };
    match serde_json::to_string_pretty(&dump) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("failed to serialise AX rows: {e}"),
    }
}

// ── Entry ───────────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_pet(app);
        }))
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            ping,
            get_snapshot,
            get_agents,
            refresh,
            dismiss_task,
            restore_dismissed_task,
            clear_dismissed,
            jump_to_task,
            get_pet_state,
            set_expanded,
            request_collapse,
            hide_pet,
            show_pet_command,
            reset_pet_position_command,
            open_permission_settings,
            get_jump_diagnostics,
            reveal_app_in_finder,
            get_settings,
            set_settings,
            reset_settings,
            get_settings_meta,
            get_custom_icons,
            save_custom_icon,
            clear_custom_icon,
            import_legacy_settings,
            open_settings,
            close_settings,
            open_data_dir,
        ])
        .setup(|app| {
            // Menu-bar/tray-only app: no Dock icon, no app switcher entry.
            // The bundled Info.plist also sets LSUIElement; this covers `cargo run`.
            // Re-asserted after the window is mapped because AppKit otherwise
            // promotes the process back to `.regular` during first activation.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let win = app.get_webview_window("main").ok_or("main window missing")?;

            let (initial_settings, corrupt_backup) = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                (rt.settings.clone(), rt.settings_corrupt_backup.clone())
            };
            if let Some(path) = corrupt_backup {
                eprintln!("settings: kept corrupt settings backup at {}", path.display());
            }

            apply_theme(&handle, initial_settings.appearance);

            // Initial position from persisted prefs, constrained to a live screen.
            let (screens, idx, prefs, hidden) = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                let screens = build_screens(&win);
                let idx = pick_screen(&screens, 0.0, 0.0);
                (screens, idx, rt.prefs.clone(), rt.prefs.hidden)
            };
            let scale = screens[idx].scale;
            let size = f64::from(initial_settings.collapsed_size) * scale;
            let rect = restore_rect(&prefs.position, (size, size), &screens);
            // macOS ignores set_position until the window has been mapped, so
            // map first, then place it, and only afterwards build the real mode.
            let _ = win.show();
            set_rect(&win, rect);
            remember_rect(&handle, rect);
            {
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                let (screens, idx) = screen_for(&win, &rect);
                rt.prefs.position = position_from_rect(&rect, &screens[idx], prefs.position.edge);
                rt.state.edge = prefs.position.edge;
                rt.state.mode = if hidden {
                    PetMode::EdgeHidden
                } else {
                    PetMode::Collapsed
                };
                rt.state.hidden = hidden;
                rt.suppress_until_ms = now_millis() + 600;
            }
            let _ = win.set_visible_on_all_workspaces(true);
            if hidden {
                if initial_settings.edge_hide_enabled {
                    enter_mode(&handle, PetMode::EdgeHidden, false);
                } else {
                    let view = {
                        let state = app.state::<AppState>();
                        let mut rt = state.inner.lock().unwrap();
                        rt.state.mode = PetMode::TrayHidden;
                        rt.state.hidden = true;
                        rt.prefs.hidden = true;
                        rt.store.save(&rt.prefs);
                        state_view(&rt.state, &rt.settings)
                    };
                    let _ = handle.emit("pet-state", view);
                    let _ = win.hide();
                }
            }

            setup_tray(&handle)?;

            // React to user drags and display changes.
            let app_for_events = handle.clone();
            win.on_window_event(move |event| match event {
                tauri::WindowEvent::Moved(_) => handle_window_moved(app_for_events.clone()),
                tauri::WindowEvent::ScaleFactorChanged { .. } => {
                    reconcile_screens(&app_for_events)
                }
                _ => {}
            });

            // First collection before the UI asks for a snapshot.
            {
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                let retention = rt.settings.retention_secs();
                rt.monitor.scan(true, retention);
            }
            let initial = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.monitor.snapshot().clone()
            };
            update_tray_tooltip(&handle, &initial);
            #[cfg(target_os = "macos")]
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    for delay in [250u64, 1200] {
                        std::thread::sleep(Duration::from_millis(delay));
                        let _ = h.set_activation_policy(tauri::ActivationPolicy::Accessory);
                    }
                });
            }
            // Debug/acceptance aid: force a start mode to verify window geometry
            // without synthetic clicks. Not used by normal launches.
            match std::env::var("AGENT_MONITOR_START_MODE").as_deref() {
                Ok("expanded") => enter_mode(&handle, PetMode::Expanded, false),
                Ok("edge_hidden") => enter_mode(&handle, PetMode::EdgeHidden, false),
                Ok("tray_hidden") => hide_pet_to_tray(&handle),
                Ok("collapsed") => enter_mode(&handle, PetMode::Collapsed, false),
                _ => {}
            }

            // Debug/acceptance aid: open the settings window on launch so the
            // window list / activation policy can be probed without synthetic
            // clicks. Not used by normal launches.
            if std::env::var("AGENT_MONITOR_OPEN_SETTINGS").as_deref() == Ok("1") {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(700));
                    show_settings_window(&h);
                });
            }

            // Scheduler: file events + settings changes share one wake channel.
            let rx = app
                .state::<AppState>()
                .wake_rx
                .lock()
                .unwrap()
                .take()
                .ok_or("scheduler wake channel already consumed")?;
            let tx = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.wake_tx.clone()
            };
            start_watcher(tx);
            start_worker(handle.clone(), rx);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── Unit tests (pure helpers only; Tauri plumbing needs a real app) ──

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(agent: &str, title: &str, status: &str) -> PendingAlert {
        PendingAlert {
            task_id: format!("{agent}:1"),
            agent_key: agent.to_lowercase(),
            agent_name: agent.into(),
            title: title.into(),
            status: status.into(),
            status_reason: "1 active user question(s)".into(),
        }
    }

    #[test]
    fn attention_alert_text_names_the_agent_for_one_question() {
        let alerts = vec![alert("DSH", "多窗口询问提醒与跳转", "waiting_input")];
        let (title, body) = attention_alert_text(&alerts, true);
        assert_eq!(title, "等待你的回答");
        assert_eq!(body, "DSH · 多窗口询问提醒与跳转");
        let (title, body) = attention_alert_text(&alerts, false);
        assert_eq!(title, "Waiting for your answer");
        assert_eq!(body, "DSH · 多窗口询问提醒与跳转");
    }

    #[test]
    fn attention_alert_text_collapses_a_batch_into_one_message() {
        let alerts = vec![
            alert("DSH", "q1", "waiting_input"),
            alert("Codex", "q2", "waiting_approval"),
            alert("ZCode", "q3", "waiting_input"),
        ];
        assert_eq!(attention_alert_text(&alerts, true).1, "3 个会话在等你回答");
        assert_eq!(
            attention_alert_text(&alerts, false).1,
            "3 sessions are waiting for you"
        );
    }

    #[test]
    fn notification_script_escapes_strings_and_controls_the_sound() {
        let script = question_notification_script(
            "等待你的回答",
            "a\"b\\c\nd",
            true,
        );
        assert_eq!(
            script,
            "display notification \"a\\\"b\\\\c d\" with title \"等待你的回答\" sound name \"Glass\""
        );
        let silent = question_notification_script("t", "b", false);
        assert!(!silent.contains("sound name"));
        assert!(silent.contains("display notification \"b\" with title \"t\""));
    }

    #[test]
    fn tray_tooltip_mentions_waiting_questions() {
        assert_eq!(
            tray_tooltip_label(0, 0, 0, true),
            "Agent Monitor — 暂无会话"
        );
        assert_eq!(
            tray_tooltip_label(3, 1, 1, false),
            "Agent Monitor — 3 session(s), 1 waiting for you"
        );
        assert_eq!(
            tray_tooltip_label(3, 1, 1, true),
            "Agent Monitor — 3 个会话，1 个待回答"
        );
        // Failures still count as attention even when questions are present.
        assert_eq!(
            tray_tooltip_label(3, 2, 1, false),
            "Agent Monitor — 3 session(s), 1 waiting for you, 2 need attention"
        );
        assert_eq!(
            tray_tooltip_label(3, 0, 0, false),
            "Agent Monitor — 3 session(s)"
        );
        assert_eq!(
            tray_tooltip_label(3, 2, 0, true),
            "Agent Monitor — 3 个会话，2 个待处理"
        );
    }
}
