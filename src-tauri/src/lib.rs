//! Agent Monitor — desktop-pet task monitor for multiple coding agents.
//!
//! Responsibilities are split into modules:
//!   * `agents/*`  — one adapter per real local data source
//!   * `monitor`   — collects adapters into one deduplicated snapshot
//!   * `jump`      — graded, verified jump execution (L3→L0)
//!   * `pet`       — pure geometry + placement persistence
//!   * `settings`  — user configuration: defaults, validation, atomic storage
//!   * `update`    — in-app updates (tauri-plugin-updater) commands/events
//!   * this file   — Tauri window/tray/command plumbing only

pub mod agents;
pub mod ax;
pub mod icons;
pub mod island;
pub mod jump;
pub mod model;
pub mod monitor;
pub mod pet;
pub mod pet_engine;
pub mod settings;
pub mod update;
#[cfg(target_os = "windows")]
mod windows;

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
use settings::{Animations, Appearance, Language, PetKind, Settings, SettingsGroup, SettingsStore};
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
    /// Derived from the webview's authoritative, persisted per-task read state.
    unread_completions: Vec<ViewedCompletion>,
    /// Wakes the scheduler immediately (file watcher events + settings changes).
    wake_tx: Sender<()>,
    /// Roaming-mascot state (Rust pet engine); idle unless `pet_kind = mascot`.
    mascot: pet_engine::mascot::MascotHost,
    /// True while the pet window's vibrancy is cleared for mascot mode.
    pet_effects_off: bool,
    /// Consecutive fallback-dismissal checks while the expanded panel is
    /// unfocused with the cursor elsewhere.
    expanded_dismiss_grace: u32,
    /// Island mode: true while the notch panel is expanded (drives
    /// repositioning when displays change).
    island_expanded: bool,
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
                unread_completions: Vec::new(),
                wake_tx,
                mascot: pet_engine::mascot::MascotHost::new("nailong"),
                pet_effects_off: false,
                expanded_dismiss_grace: 0,
                island_expanded: false,
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
    let rect = { state.inner.lock().unwrap().last_rect };
    rect.unwrap_or_else(|| window_rect(win))
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
    edge: Edge,
) -> (f64, f64) {
    match mode {
        PetMode::Expanded => (
            EXPANDED_WIDTH * scale,
            expanded_height_logical(rows) * scale,
        ),
        PetMode::EdgeHidden => match edge {
            Edge::Top | Edge::Bottom => (collapsed_size * scale, sliver),
            _ => (sliver, collapsed_size * scale),
        },
        PetMode::Collapsed | PetMode::TrayHidden => {
            (collapsed_size * scale, collapsed_size * scale)
        }
        PetMode::Dragging => (current.w, current.h),
    }
}

fn animate(win: &WebviewWindow, from: Rect, to: Rect, enabled: bool) {
    let generation = ANIMATION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !enabled {
        set_rect(win, to);
        return;
    }
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
        // The island stays anchored to the menu bar while its sheet unfolds.
        let steps = if win.label() == "island" {
            if to.h > from.h { 20 } else { 12 }
        } else { 7 };
        for i in 1..=steps {
            if ANIMATION_GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            let t = i as f64 / steps as f64;
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
            std::thread::sleep(Duration::from_millis(if win.label() == "island" { 16 } else { 18 }));
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
    if std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok() {
        eprintln!("mascot-debug: enter_mode {:?}", mode);
    }
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
    // Mascot pets use the CLASSIC geometry pipeline end to end (snap, edge
    // hide, sliver) — "the pet is just different artwork" — so there is no
    // special collapsed branch here anymore.
    let edge = if edge == Edge::None {
        nearest_edge(&current, &visible)
    } else {
        edge
    };
    let sliver = f64::from(settings.hidden_sliver) * scale;
    let (tw, th) = target_size(mode, rows, collapsed_size, sliver, scale, current, edge);
    let base = Rect::new(current.x, current.y, tw, th);
    let target = match mode {
        // The tab keeps the pet's position along the edge, so expanding again
        // grows from where the user left it.
        PetMode::EdgeHidden => hide_tab_rect(current, &visible, edge, sliver, collapsed_size * scale),
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
        // Screen queries wait for the UI thread, whose window callbacks take
        // this lock. Reuse the screen data collected before acquiring it.
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
    animate(&win, current, target, animated && animate_window);
    persist(app, target, edge, mode);
}

/// Keep the pet inside a connected screen after display changes / unplug.
fn reconcile_screens(app: &AppHandle) {
    // The island follows its own display (the notched one), independent of
    // where the pet lives.
    #[cfg(target_os = "macos")]
    refresh_island_rect(app);
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let current = tracked_rect(app, &win);
    let (screens, idx) = screen_for(&win, &current);
    if screens.is_empty() {
        return;
    }
    let visible = screens[idx].visible;
    let (mode, edge, sliver, collapsed_size) = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        (
            rt.state.mode,
            rt.state.edge,
            f64::from(rt.settings.hidden_sliver),
            f64::from(rt.settings.collapsed_size),
        )
    };
    if mode == PetMode::Dragging || mode == PetMode::TrayHidden {
        return;
    }
    let target = if mode == PetMode::EdgeHidden && edge != Edge::None {
        hide_tab_rect(current, &visible, edge, sliver * screens[idx].scale, collapsed_size * screens[idx].scale)
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
        rt.mascot.invalidate();
        rt.state.edge = Edge::Right;
        rt.state.mode = PetMode::Collapsed;
        rt.state.hidden = false;
        rt.prefs.hidden = false;
        rt.suppress_until_ms = now_millis() + 300;
        rt.store.save(&rt.prefs);
    }
    let _ = win.show();
    remember_rect(app, rect);
    animate(&win, window_rect(&win), rect, false);
    persist(app, rect, Edge::Right, PetMode::Collapsed);
    let view = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        state_view(&rt.state, &rt.settings)
    };
    let _ = app.emit("pet-state", view);
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

    if previous.island_mode != current.island_mode {
        #[cfg(target_os = "macos")]
        enter_island_mode(app, current.island_mode);
        update_tray_menu(app);
    }

    if mode == PetMode::EdgeHidden && previous.edge_hide_enabled && !current.edge_hide_enabled {
        // The feature that parked the window off-screen was removed: keep the
        // window reachable (tray-hidden) instead of leaving an unreachable sliver.
        hide_pet_to_tray(app);
    } else if mode == PetMode::Expanded || mode == PetMode::EdgeHidden {
        let geometry_changed = previous.max_visible_rows != current.max_visible_rows
            || previous.hidden_sliver != current.hidden_sliver
            || previous.collapsed_size != current.collapsed_size
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

// ── Dynamic-Island mode ─────────────────────────────────────────

/// Ear width flanking the notch hardware on each side (logical pt). The
/// collapsed pill = notch + two ears: the island reads as if it had grown out
/// of the hardware notch, content living only in the ears.
const ISLAND_EAR_PT: f64 = 160.0;
/// Expanded panel width (logical pt).
const ISLAND_PANEL_W_PT: f64 = 520.0;
/// Expanded panel height bounds (logical pt); the frontend owns the exact
/// value within these.
const ISLAND_PANEL_MIN_H_PT: f64 = 96.0;
const ISLAND_PANEL_MAX_H_PT: f64 = 680.0;

fn animations_enabled(settings: &Settings) -> bool {
    !matches!(settings.animations, Animations::Off)
}

/// Create the island window on first use; hidden until island mode turns on.
#[cfg(target_os = "macos")]
fn ensure_island_window(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(win) = app.get_webview_window("island") {
        return Some(win);
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "island",
        tauri::WebviewUrl::App("island.html".into()),
    )
    .title("Agent Monitor Island")
    .inner_size(200.0, 38.0)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .accept_first_mouse(true)
    .visible(false)
    .visible_on_all_workspaces(true)
    .build()
    .ok()?;
    island::apply_panel_style(&win).ok()?;
    window_vibrancy::apply_vibrancy(
        &win,
        window_vibrancy::NSVisualEffectMaterial::Popover,
        Some(window_vibrancy::NSVisualEffectState::Active),
        Some(24.0),
    )
    .inspect_err(|error| eprintln!("island material: {error}"))
    .ok()?;
    Some(win)
}

/// Physical-pixel rect for the island at a logical size, centred on the notch.
/// Matches the notched `NSScreen` (pt, top-left origin) to a tao monitor (px)
/// so the pill lands on the right display in any arrangement.
#[cfg(target_os = "macos")]
fn island_rect(app: &AppHandle, m: island::IslandMetrics, w_pt: f64, h_pt: f64) -> Option<Rect> {
    let win = app
        .get_webview_window("island")
        .or_else(|| app.get_webview_window("main"))?;
    // The pill is computed in global Cocoa points (what NSScreen reports),
    // then expressed in tao's "physical" space, which on macOS is point ×
    // **the scale of the screen the window is on right now** — tao converts
    // back with that same factor, so the window lands exactly on the Cocoa
    // point regardless of which display it currently sits on. Matching the
    // target monitor by position and scaling by *its* factor double-scales
    // the first placement on mixed-scale multi-display setups (observed: the
    // island parked ~5000 pt off-screen when the notched MacBook display woke
    // up next to a 1× external display).
    let scale = win.scale_factor().unwrap_or(1.0);
    let x = (m.center_x - w_pt / 2.0) * scale;
    let y = m.origin_y * scale;
    Some(Rect::new(x, y, w_pt * scale, h_pt * scale))
}

/// Apply the current island mode. The island is **additive**: the pet keeps
/// its own visibility state machine (tray hide, edge hide, auto-hide and the
/// mascot all keep working untouched), so toggling the island never touches
/// the pet and both can be on screen at once.
#[cfg(target_os = "macos")]
fn enter_island_mode(app: &AppHandle, enabled: bool) {
    if !enabled {
        if let Some(win) = app.get_webview_window("island") {
            let _ = win.hide();
        }
        return;
    }
    let Some(metrics) = island::island_display_geometry() else {
        // No usable display (never reached in practice): keep the setting as
        // the user chose; the next display event re-runs this path.
        return;
    };
    let Some(win) = ensure_island_window(app) else { return };
    let w_pt = metrics.notch_w + 2.0 * ISLAND_EAR_PT;
    if let Some(rect) = island_rect(app, metrics, w_pt, metrics.bar_h) {
        set_rect(&win, rect);
    }
    {
        let state = app.state::<AppState>();
        let mut rt = state.inner.lock().unwrap();
        rt.island_expanded = false;
    }
    let _ = win.set_shadow(false);
    let _ = win.show();
    let _ = app.emit("island-geometry", serde_json::json!({ "mode": "collapsed", "metrics": metrics }));
    start_island_hover_watcher(app);
}

/// Cursor-hover watcher for the island. The webview's `mouseenter` is not
/// reliably delivered for a borderless status-bar-level window, so hover is
/// detected here — same global-cursor source as the pet's edge logic — and
/// forwarded to the island page as enter/leave edges. The thread parks itself
/// (80 ms tick) and exits once island mode is off.
#[cfg(target_os = "macos")]
fn start_island_hover_watcher(app: &AppHandle) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut last_inside: Option<bool> = None;
        loop {
            std::thread::sleep(Duration::from_millis(80));
            let enabled = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.settings.island_mode
            };
            if !enabled {
                RUNNING.store(false, Ordering::SeqCst);
                return;
            }
            let Some(win) = app.get_webview_window("island") else { continue };
            if !win.is_visible().unwrap_or(false) {
                continue;
            }
            let Some((cx, cy)) = cursor_point() else { continue };
            let scale = win.scale_factor().unwrap_or(1.0);
            let pos = win.outer_position().unwrap_or(PhysicalPosition::new(0, 0));
            let size = win.outer_size().unwrap_or(PhysicalSize::new(0, 0));
            let inside = cx * scale >= f64::from(pos.x)
                && cx * scale <= f64::from(pos.x + size.width as i32)
                && cy * scale >= f64::from(pos.y)
                && cy * scale <= f64::from(pos.y + size.height as i32);
            if last_inside != Some(inside) {
                last_inside = Some(inside);
                let _ = app.emit("island-hover", inside);
            }
        }
    });
}

/// Re-place the island window at its current mode's rect (display changes,
/// scale changes). Height while expanded belongs to the frontend and is kept.
#[cfg(target_os = "macos")]
fn refresh_island_rect(app: &AppHandle) {
    let expanded = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        if !rt.settings.island_mode {
            return;
        }
        rt.island_expanded
    };
    let Some(metrics) = island::island_display_geometry() else { return };
    let Some(win) = app.get_webview_window("island") else { return };
    let rect = if expanded {
        let cur = window_rect(&win);
        island_rect(
            app,
            metrics,
            ISLAND_PANEL_W_PT,
            (cur.h / metrics.scale).max(ISLAND_PANEL_MIN_H_PT),
        )
    } else {
        island_rect(app, metrics, metrics.notch_w + 2.0 * ISLAND_EAR_PT, metrics.bar_h)
    };
    if let Some(rect) = rect {
        set_rect(&win, rect);
    }
    let _ = app.emit("island-geometry", serde_json::json!({ "metrics": metrics }));
}

/// Tray → "Check for Updates…": open Settings on the Updates pane.
///
/// No network work happens here (the menu must never block); the frontend pane
/// runs `check_for_update` itself when it becomes visible. A freshly created
/// window has no listener yet, so the event is repeated once its local page has
/// loaded — `open-update-pane` is idempotent for the UI.
fn open_update_pane(app: &AppHandle) {
    let existed = app.get_webview_window("settings").is_some();
    show_settings_window(app);
    let _ = app.emit("open-update-pane", ());
    if !existed {
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1200));
            let _ = handle.emit("open-update-pane", ());
        });
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct ViewedCompletion {
    task_id: String,
    completion_id: String,
}

#[tauri::command]
fn watch_unread_completions(state: State<'_, AppState>, completions: Vec<ViewedCompletion>) {
    let mut rt = state.inner.lock().unwrap();
    rt.unread_completions = completions;
    let _ = rt.wake_tx.send(());
}

fn normalized_conversation_title(title: &str) -> String {
    title.trim().trim_start_matches('#').split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn viewed_completion(context: &ax::FocusedConversation, tasks: &[model::TaskRecord], unread: &[ViewedCompletion]) -> Option<ViewedCompletion> {
    let in_app: Vec<_> = tasks.iter().filter(|task|
        task.target.bundle_id.as_deref() == Some(context.bundle_id.as_str())
    ).collect();
    let by_id: Vec<_> = in_app.iter().copied().filter(|task|
        task.session_id.as_deref().into_iter().chain(task.task_id.as_deref()).any(|id|
            context.references.iter().any(|reference|
                reference.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').any(|part| part == id)))
    ).collect();
    let candidates: Vec<_> = if !by_id.is_empty() { by_id } else {
        in_app.into_iter().filter(|task| {
            let title = normalized_conversation_title(&task.title);
            let app = context.app_name.as_str();
            let focused = [" — ", " – ", " - "].iter().find_map(|separator|
                context.title.strip_suffix(&format!("{separator}{app}")))
                .map(normalized_conversation_title).unwrap_or_else(|| normalized_conversation_title(&context.title));
            !title.is_empty() && (title == focused || context.selected_titles.iter().any(|text| normalized_conversation_title(text) == title))
        }).collect()
    };
    if candidates.len() != 1 { return None; }
    let task = candidates[0];
    if task.status != model::TaskStatus::Completed { return None; }
    unread.iter().find(|item| item.task_id == task.id && task.completion_id.as_deref() == Some(item.completion_id.as_str())).cloned()
}

fn terminal_viewed_completion(paths: &[PathBuf], tasks: &[model::TaskRecord], unread: &[ViewedCompletion]) -> Option<ViewedCompletion> {
    let candidates: Vec<_> = tasks.iter().filter(|task|
        task.agent_key == "codex" && task.host == "terminal" && task.session_id.as_deref().is_some_and(|id|
            paths.iter().any(|path| path.file_name().and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&format!("-{id}.jsonl")))))
    ).collect();
    if candidates.len() != 1 { return None; }
    let task = candidates[0];
    if task.status != model::TaskStatus::Completed { return None; }
    unread.iter().find(|item| item.task_id == task.id
        && task.completion_id.as_deref() == Some(item.completion_id.as_str())).cloned()
}

/// Reuse the pet heartbeat for a fast foreground check; OS queries run off the
/// animation thread, with at most one probe running. No task scan or UI timer.
fn schedule_terminal_read_check(app: &AppHandle) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let pending = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.unread_completions.iter().any(|item| rt.monitor.snapshot().tasks.iter().any(|task|
            task.id == item.task_id && task.agent_key == "codex" && task.host == "terminal"))
    };
    if !pending || ax::foreground_bundle_id().as_deref() != Some("com.apple.Terminal") { return; }
    if RUNNING.swap(true, Ordering::SeqCst) { return; }
    let app = app.clone();
    std::thread::spawn(move || {
        struct Reset;
        impl Drop for Reset { fn drop(&mut self) { RUNNING.store(false, Ordering::SeqCst); } }
        let _reset = Reset;
        let Some(tty) = jump::focused_terminal_tty() else { return; };
        let paths = agents::codex::terminal_rollouts(&tty);
        if std::env::var_os("AGENT_MONITOR_DEBUG_READS").is_some() {
            eprintln!("terminal-read: tty={tty} rollouts={}", paths.len());
        }
        if paths.is_empty() { return; }
        let viewed = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            terminal_viewed_completion(&paths, &rt.monitor.snapshot().tasks, &rt.unread_completions)
        };
        let Some(viewed) = viewed else { return; };
        // A tab switch during lsof must not acknowledge the previous tab.
        if jump::focused_terminal_tty().as_deref() != Some(tty.as_str()) { return; }
        app.state::<AppState>().inner.lock().unwrap().unread_completions.retain(|item| item != &viewed);
        let _ = app.emit("completion-viewed", viewed);
    });
}

fn acknowledge_viewed_completions(app: &AppHandle) {
    let (mut unread, tasks) = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        if rt.unread_completions.is_empty() { return; }
        (rt.unread_completions.clone(), rt.monitor.snapshot().tasks.clone())
    };
    let pending: Vec<_> = tasks.iter().filter(|task| unread.iter().any(|item|
        item.task_id == task.id && task.completion_id.as_deref() == Some(item.completion_id.as_str())
    )).cloned().collect();
    for (task_id, completion_id) in agents::codex::read_completions(&pending) {
        let viewed = ViewedCompletion { task_id, completion_id };
        unread.retain(|item| item != &viewed);
        app.state::<AppState>().inner.lock().unwrap().unread_completions.retain(|item| item != &viewed);
        let _ = app.emit("completion-viewed", viewed);
    }
    if unread.is_empty() { return; }
    let bundles: Vec<String> = tasks.iter().filter(|task| unread.iter().any(|item| item.task_id == task.id))
        .filter_map(|task| task.target.bundle_id.clone()).collect();
    let Some(context) = ax::focused_conversation(&bundles) else { return; };
    if let Some(viewed) = viewed_completion(&context, &tasks, &unread) {
        app.state::<AppState>().inner.lock().unwrap().unread_completions.retain(|item| item != &viewed);
        let _ = app.emit("completion-viewed", viewed);
    }
}

#[tauri::command]
fn get_agents(state: State<'_, AppState>) -> Vec<AgentDescriptor> {
    state.inner.lock().unwrap().monitor.descriptors()
}

#[tauri::command]
fn set_task_name(app: AppHandle, state: State<'_, AppState>, task_id: String, name: String) -> Result<Snapshot, String> {
    let snapshot = {
        let mut rt = state.inner.lock().unwrap();
        rt.monitor.set_custom_name(&task_id, &name)?;
        rt.monitor.snapshot().clone()
    };
    let _ = app.emit("tasks-updated", &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_task_pinned(app: AppHandle, state: State<'_, AppState>, task_id: String, pinned: bool) -> Result<Snapshot, String> {
    let snapshot = {
        let mut rt = state.inner.lock().unwrap();
        rt.monitor.set_pinned(&task_id, pinned)?;
        let retention = rt.settings.retention_secs();
        rt.monitor.scan(true, retention);
        rt.monitor.snapshot().clone()
    };
    let _ = app.emit("tasks-updated", &snapshot);
    update_tray_tooltip(&app, &snapshot);
    drain_attention(&app);
    wake(&app);
    Ok(snapshot)
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
    let tasks = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        rt.monitor.snapshot().tasks.clone()
    };
    let outcome = match tasks.iter().find(|task| task.id == task_id) {
        Some(task) => jump::jump(task, &tasks),
        None => JumpOutcome::failed(&task_id, JumpLevel::L0, "task_gone"),
    };
    acknowledge_viewed_completions(&app);
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
        }
        if focus == Some(true) {
            if let Some(win) = app.get_webview_window("main") {
                // Explicit clicks, including rename, must receive keyboard input.
                let _ = win.set_focus();
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
fn hide_pet(app: AppHandle, automatic: Option<bool>) {
    if automatic == Some(true) {
        let can_hide = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            rt.settings.edge_hide_enabled && rt.settings.auto_hide_after_s > 0
                && rt.state.mode == PetMode::Collapsed && rt.state.edge != Edge::None
        };
        if can_hide { hide_pet_to_edge(&app); }
    } else {
        hide_pet_now(&app);
    }
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

/// Ask macOS to add the CURRENT app to Accessibility (native AX prompt). Returns
/// `true` when already trusted, else `false` after requesting the prompt; the
/// Settings UI re-checks on focus.
#[tauri::command]
async fn request_accessibility_permission() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(ax::request_accessibility_permission)
        .await
        .unwrap_or_else(|_| Err("accessibility_prompt_failed".to_string()))
}

/// Clear this bundle's stale TCC records (Accessibility + AppleEvents) so the
/// user can re-add the current copy. Explicit user action only; never automatic.
#[tauri::command]
async fn reset_app_permissions() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(jump::reset_app_permissions)
        .await
        .unwrap_or_else(|_| Err("permission_reset_failed".to_string()))
}

// ── Island commands ─────────────────────────────────────────────

#[tauri::command]
fn island_metrics() -> Option<island::IslandMetrics> {
    #[cfg(target_os = "macos")]
    {
        island::island_display_geometry()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Grow the window to the expanded panel rect (the frontend supplies its
/// desired content height in logical pt). Returns false when absent.
#[tauri::command]
fn island_expand(app: AppHandle, h_pt: f64, motion: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(metrics) = island::island_display_geometry() else {
            return false;
        };
        let Some(win) = app.get_webview_window("island") else {
            return false;
        };
        if !h_pt.is_finite() { return false; }
        let h = h_pt.clamp(ISLAND_PANEL_MIN_H_PT, ISLAND_PANEL_MAX_H_PT.min(metrics.screen_h - 60.0));
        let Some(target) = island_rect(&app, metrics, ISLAND_PANEL_W_PT, h) else {
            return false;
        };
        let enabled = {
            let state = app.state::<AppState>();
            let mut rt = state.inner.lock().unwrap();
            rt.island_expanded = true;
            motion && animations_enabled(&rt.settings)
        };
        let _ = win.set_shadow(true);
        animate(&win, window_rect(&win), target, enabled);
        let _ = app.emit(
            "island-geometry",
            serde_json::json!({ "mode": "expanded", "w_pt": ISLAND_PANEL_W_PT, "h_pt": h }),
        );
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, h_pt, motion);
        false
    }
}

/// Shrink back to the notch pill.
#[tauri::command]
fn island_collapse(app: AppHandle, motion: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(metrics) = island::island_display_geometry() else {
            return false;
        };
        let Some(win) = app.get_webview_window("island") else {
            return false;
        };
        let Some(target) =
            island_rect(&app, metrics, metrics.notch_w + 2.0 * ISLAND_EAR_PT, metrics.bar_h)
        else {
            return false;
        };
        let enabled = {
            let state = app.state::<AppState>();
            let mut rt = state.inner.lock().unwrap();
            rt.island_expanded = false;
            motion && animations_enabled(&rt.settings)
        };
        animate(&win, window_rect(&win), target, enabled);
        let _ = win.set_shadow(false);
        let _ = app.emit("island-geometry", serde_json::json!({ "mode": "collapsed" }));
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, motion);
        false
    }
}

#[cfg_attr(target_os = "windows", allow(unused_variables))]
#[tauri::command]
fn set_island_mode(app: AppHandle, enabled: bool) -> Settings {
    let mut next = settings_snapshot(&app);
    next.island_mode = enabled;
    commit_settings(&app, next)
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
    platform: &'static str,
    local_build: bool,
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
        platform: std::env::consts::OS,
        local_build: update::require_release_build().is_err(),
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

// ── Mascot pack commands (docs/PETS.md) ─────────────────────────

#[tauri::command]
fn list_pet_packs() -> Vec<pet_engine::backend::BackendInfo> {
    pet_engine::mascot::list_packs()
}

/// Frontend render-chain telemetry (headless diagnosis). Only printed while
/// `AGENT_MONITOR_DEBUG_MASCOT` is set.
#[tauri::command]
fn pet_debug_log(msg: String) {
    if std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok() {
        eprintln!("pet-render: {msg}");
    }
}

#[tauri::command]
fn get_pet_pack_image(pack: String, image: String) -> Result<String, String> {
    use base64::Engine as _;
    let bytes = pet_engine::mascot::pack_image(&pack, &image)
        .ok_or_else(|| format!("帧 {image} 不在宠物包 {pack} 中"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[derive(serde::Deserialize)]
struct IncomingPackFile {
    path: String,
    /// base64 of the file content (the webview reads folders directly).
    data_base64: String,
}

#[tauri::command]
fn import_pet_pack(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    files: Vec<IncomingPackFile>,
) -> Result<pet_engine::backend::BackendInfo, String> {
    use base64::Engine as _;
    let incoming: Vec<pet_engine::mascot::IncomingFile> = files
        .into_iter()
        .map(|f| {
            Ok(pet_engine::mascot::IncomingFile {
                path: f.path,
                bytes: base64::engine::general_purpose::STANDARD
                    .decode(f.data_base64.trim())
                    .map_err(|e| format!("base64 解码失败: {e}"))?,
            })
        })
        .collect::<Result<_, String>>()?;
    let info = pet_engine::mascot::import_pack(&name, &incoming)?;
    // A settings switch to the new pack must see it immediately.
    state.inner.lock().unwrap().mascot.invalidate();
    let _ = app.emit("pet-packs-changed", ());
    Ok(info)
}

/// Right-click action menu: trigger a named action the pack actually ships.
#[tauri::command]
fn set_mascot_action(state: State<'_, AppState>, action: String) -> bool {
    let mut rt = state.inner.lock().unwrap();
    rt.mascot.perform(&action)
}

/// Right-click menu utility: walk the mascot back to the middle of the floor.
#[tauri::command]
fn pet_center(state: State<'_, AppState>) -> bool {
    let mut rt = state.inner.lock().unwrap();
    rt.mascot.center();
    true
}

/// Right-click action menu: a NATIVE popup at the cursor. Zero geometry
/// impact — the pet never shifts, snaps or clips because of a menu.
#[tauri::command]
fn pet_menu_open(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let (ready, lang) = {
        let rt = state.inner.lock().unwrap();
        (
            matches!(rt.state.mode, PetMode::Collapsed | PetMode::EdgeHidden),
            resolved_language(&rt.settings),
        )
    };
    if !ready {
        return Ok(false);
    }
    let menu_actions = {
        let rt = state.inner.lock().unwrap();
        if rt.settings.pet_kind == PetKind::Mascot { rt.mascot.menu_actions() } else { Vec::new() }
    };
    let roaming = {
        let rt = state.inner.lock().unwrap();
        rt.settings.pet_kind == PetKind::Mascot && rt.mascot.ready()
            && rt.mascot.window_policy() == pet_engine::backend::WindowPolicy::FollowsAnchor
    };
    let Some(win) = app.get_webview_window("main") else {
        return Ok(false);
    };
    let menu = tauri::menu::Menu::new(&app).map_err(|e| e.to_string())?;
    for entry in &menu_actions {
        let item = tauri::menu::MenuItem::with_id(
            &app,
            format!("pet-action:{}", entry.action),
            pet_menu_label(&entry.label_key, lang),
            true,
            None::<&str>,
        ).map_err(|e| e.to_string())?;
        menu.append(&item).map_err(|e| e.to_string())?;
    }
    if roaming {
        let item = tauri::menu::MenuItem::with_id(
            &app,
            "pet-action:__center",
            pet_menu_label("__center", lang),
            true,
            None::<&str>,
        ).map_err(|e| e.to_string())?;
        menu.append(&item).map_err(|e| e.to_string())?;
    }
    if !menu_actions.is_empty() || roaming {
        menu.append(&tauri::menu::PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    for action in ["__tasks", "__keep_visible", "__reset", "__settings", "__hide"] {
        let item = tauri::menu::MenuItem::with_id(
            &app, format!("pet-action:{action}"), pet_menu_label(action, lang), true, None::<&str>,
        ).map_err(|e| e.to_string())?;
        menu.append(&item).map_err(|e| e.to_string())?;
    }
    // Pop up at the cursor. popup_at's position is RELATIVE TO THE WINDOW's
    // top-left corner (docs) in logical units — convert the global cursor
    // point through the window's physical rect and its screen scale.
    let rect = tracked_rect(&app, &win);
    let (screens, idx) = screen_for(&win, &rect);
    let scale = if screens.is_empty() { 2.0 } else { screens[idx].scale };
    let (cx, cy) = cursor_point().unwrap_or((0.0, 0.0));
    let rel_x = (cx - rect.x / scale).clamp(0.0, rect.w / scale);
    let rel_y = (cy - rect.y / scale).clamp(0.0, rect.h / scale);
    win.popup_menu_at(
        &menu,
        tauri::Position::Logical(tauri::LogicalPosition::new(rel_x, rel_y)),
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Localized labels for the pet's native menu (backend supplies i18n keys).
fn pet_menu_label(label_key: &str, lang: settings::Language) -> String {
    let zh = lang == settings::Language::Zh;
    match (label_key, zh) {
        ("menuJump", false) => "Jump".into(),
        ("menuJump", true) => "跳一下".into(),
        ("menuRun", false) => "Run".into(),
        ("menuRun", true) => "跑步".into(),
        ("menuWalk", false) => "Walk a bit".into(),
        ("menuWalk", true) => "走两步".into(),
        ("menuBounce", false) => "Celebrate".into(),
        ("menuBounce", true) => "撒个欢".into(),
        ("menuSit", false) => "Sit down".into(),
        ("menuSit", true) => "坐下".into(),
        ("menuTrip", false) => "Stumble".into(),
        ("menuTrip", true) => "摔一跤".into(),
        ("menuStand", false) => "Stand still".into(),
        ("menuStand", true) => "站好".into(),
        ("menuPet", false) => "Head pat".into(),
        ("menuPet", true) => "摸摸头".into(),
        ("menuNap", false) => "Nap".into(),
        ("menuNap", true) => "打个盹".into(),
        ("menuCelebrate", false) => "Celebrate".into(),
        ("menuCelebrate", true) => "庆祝一下".into(),
        ("menuWatch", false) => "Look around".into(),
        ("menuWatch", true) => "看看四周".into(),
        ("menuThink", false) => "Think for a bit".into(),
        ("menuThink", true) => "思考一下".into(),
        ("menuWork", false) => "Work together".into(),
        ("menuWork", true) => "陪我工作".into(),
        ("menuSurprise", false) => "Surprise!".into(),
        ("menuSurprise", true) => "吓一跳".into(),
        ("menuResume", false) => "Follow task status".into(),
        ("menuResume", true) => "恢复跟随任务状态".into(),
        ("__tasks", false) => "Show Tasks".into(),
        ("__tasks", true) => "打开任务列表".into(),
        ("__keep_visible", false) => "Keep Pet Visible".into(),
        ("__keep_visible", true) => "保持桌宠显示".into(),
        ("__reset", false) => "Bring Pet Back / Reset Position".into(),
        ("__reset", true) => "找回桌宠 / 重置位置".into(),
        ("__settings", false) => "Settings…".into(),
        ("__settings", true) => "设置…".into(),
        ("__hide", false) => "Hide Pet".into(),
        ("__hide", true) => "隐藏桌宠".into(),
        ("__center", false) => "Go to screen center".into(),
        ("__center", true) => "回到屏幕中间".into(),
        _ => label_key.to_string(),
    }
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
    std::fs::create_dir_all(settings::data_dir()).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(settings::data_dir(), None::<&str>)
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
fn spawn_question_alert(app: &AppHandle, alerts: Vec<PendingAlert>, notify: bool, sound: bool, zh: bool) {
    if alerts.is_empty() || (!notify && !sound) {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_notification::NotificationExt;
        let app = app.clone();
        std::thread::spawn(move || {
            if notify {
                let (title, body) = attention_alert_text(&alerts, zh);
                let builder = app.notification().builder().title(title).body(body);
                let _ = builder.sound(if sound { "Default" } else { "Silent" }).show();
            } else if sound {
                unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0); }
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
    let _ = app;
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
    spawn_question_alert(app, alerts, notify, sound, zh);
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
    let mut candidates = vec![
        home.join(".claude/projects"),
        home.join(".dsh/storages/session_projcache/sessions"),
        home.join(".dsh/sessions"),
        home.join(".zcode/v2"),
        home.join(".gemini/antigravity-cli"),
        home.join(".gemini/antigravity/conversations"),
    ];
    candidates.push(home.join(".gemini/antigravity/conversation_summaries.db"));
    candidates.extend(agents::codex::configured_homes());
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
            // Bound each burst: activity in another agent must not postpone
            // an unanswered-question update indefinitely.
            let started = std::time::Instant::now();
            while let Some(remaining) = Duration::from_millis(700).checked_sub(started.elapsed()) {
                if raw_rx.recv_timeout(remaining).is_err() { break; }
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
            // Only unread completions need foreground-view checks while hidden.
            // Once acknowledged, hidden/empty work returns to the 30 s floor.
            let interval = if (hidden || empty) && rt.unread_completions.is_empty() {
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
        acknowledge_viewed_completions(&app);
        if !(hidden && empty) {
            reconcile_screens(&app);
        }
    });
}

fn system_is_chinese() -> bool {
    #[cfg(target_os = "windows")]
    { return windows::system_is_chinese(); }
    #[cfg(not(target_os = "windows"))]
    {
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
    let update_item = MenuItemBuilder::with_id(
        "check-update",
        if zh {
            "检查更新…"
        } else {
            "Check for Updates…"
        },
    )
    .build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", if zh { "退出" } else { "Quit" }).build(app)?;
    let island_item: Option<tauri::menu::CheckMenuItem<tauri::Wry>> = {
        #[cfg(target_os = "macos")]
        {
            Some(
                tauri::menu::CheckMenuItemBuilder::with_id(
                    "island-mode",
                    if zh { "灵动岛模式" } else { "Dynamic Island" },
                )
                .checked(settings.island_mode)
                .build(app)?,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (zh, settings);
            None
        }
    };
    let mut menu = MenuBuilder::new(app)
        .item(&settings_item)
        .separator()
        .item(&show_item);
    if let Some(item) = &island_item {
        menu = menu.item(item);
    }
    menu.item(&hide_item)
        .item(&reset_item)
        .separator()
        .item(&refresh_item)
        .item(&update_item)
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
        "check-update" => open_update_pane(app),
        "show" => show_pet(app),
        "island-mode" => {
            let mut next = settings_snapshot(app);
            next.island_mode = !next.island_mode;
            commit_settings(app, next);
        }
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

// ── Mascot (roaming pet) host ───────────────────────────────────

/// Logical margin between the mascot's anchor (feet) and the window bottom.
const MASCOT_BOTTOM_MARGIN: f64 = 6.0;

/// Restore the pet window's vibrancy (mirrors tauri.conf.json: popover,
/// active, radius 16). Clearing goes through `window_vibrancy` directly —
/// tauri's `set_effects(None)` is a Windows-only no-op on macOS.
#[cfg(target_os = "macos")]
fn restore_pet_vibrancy(win: &WebviewWindow) -> Result<(), String> {
    window_vibrancy::apply_vibrancy(
        win,
        window_vibrancy::NSVisualEffectMaterial::Popover,
        Some(window_vibrancy::NSVisualEffectState::Active),
        Some(16.0),
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn restore_pet_vibrancy(_win: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

fn clear_pet_vibrancy(win: &WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    { window_vibrancy::clear_vibrancy(win).map(|_| ()).map_err(|e| e.to_string()) }
    #[cfg(not(target_os = "macos"))]
    { let _ = win; Ok(()) }
}

/// One engine tick: advance the mascot, move the window with it, hand the
/// frame to the webview. Self-pauses unless the collapsed pet is in mascot
/// mode — the classic capsule and every other mode are untouched.
fn mascot_ticker_step(app: &AppHandle) {
    static TICKS: AtomicU64 = AtomicU64::new(0);
    let step = TICKS.fetch_add(1, Ordering::Relaxed);
    if step % 25 == 0 { schedule_terminal_read_check(app); }
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let state = app.state::<AppState>();
    let scale_size_kind = {
        let rt = state.inner.lock().unwrap();
        let kind = rt.settings.pet_kind;
        let size = f64::from(rt.settings.collapsed_size);
        let mode = rt.state.mode;
        let hidden = rt.state.hidden;
        (kind, size, mode, hidden)
    };
    let (kind, size_logical, mode, hidden) = scale_size_kind;
    let debug = std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok();
    if debug && step % 50 == 0 {
        eprintln!(
            "mascot-debug: tick={} kind={:?} mode={:?} hidden={} ready={}",
            step, kind, mode, hidden, {
                let rt = state.inner.lock().unwrap();
                rt.mascot.ready()
            }
        );
    }
    // Mascot active: the engine runs and frames flow in every collapsed-like
    // state INCLUDING EdgeHidden — the sliver tab shows a cropped piece of
    // the pet, which requires the backend to be painting. Only the expanded
    // panel and tray-hidden window replace the pet. The vibrancy clear uses
    // the same condition (a roaming/fixed pet window is fully transparent;
    // the frosted material would paint over nothing and just add a card).
    // The objc calls MUST run on the main thread: window_vibrancy aborts
    // when called from a background thread, so the work is dispatched and
    // the flag flips inside the dispatched closure.
    let showing_mascot = kind == PetKind::Mascot
        && matches!(
            mode,
            PetMode::Collapsed | PetMode::EdgeHidden | PetMode::Dragging
        );
    {
        let needs_sync = state.inner.lock().unwrap().pet_effects_off != showing_mascot;
        if needs_sync {
            let app_handle = app.clone();
            let win_handle = win.clone();
            let _ = win.run_on_main_thread(move || {
                let state = app_handle.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                if rt.pet_effects_off == showing_mascot {
                    return;
                }
                let result = if showing_mascot {
                    clear_pet_vibrancy(&win_handle)
                } else {
                    restore_pet_vibrancy(&win_handle)
                };
                if std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok() {
                    eprintln!("mascot-debug: vibrancy -> {showing_mascot} result {:?}", result);
                }
                if result.is_ok() {
                    rt.pet_effects_off = showing_mascot;
                }
            });
        }
    }
    let needs_mood = {
        let rt = state.inner.lock().unwrap();
        step % 25 == 0 || (showing_mascot && (!rt.mascot.ready() || rt.mascot.pack_name != rt.settings.mascot_pack))
    };
    let task_mood = if needs_mood {
        let mut rt = state.inner.lock().unwrap();
        let tasks = rt.monitor.snapshot().tasks.clone();
        Some(rt.mascot.task_mood.update(&tasks, now_millis()))
    } else {
        None
    };
    if !showing_mascot {
        expanded_outside_dismissal(app, &win, step);
        return;
    }
    let rect = tracked_rect(app, &win);
    let (screens, idx) = screen_for(&win, &rect);
    if screens.is_empty() {
        return;
    }
    let scale = screens[idx].scale;
    let visible = screens[idx].visible;

    // Lazy activation / pack switch (settings can change the pack any time).
    // A fresh roaming mascot spawns on the floor, never mid-air where the
    // restored window happened to be. Activation also waits out the launch
    // placement window: the ticker can tick before the startup code has
    // placed the window, and spawning from a not-yet-placed rect pins the
    // pet to a screen corner.
    let anchor = pet_engine::mascot::MascotHost::anchor_of(
        (rect.x, rect.y, rect.w, rect.h),
        MASCOT_BOTTOM_MARGIN * scale,
    );
    let spawn = (anchor.0, visible.y + visible.h);
    let frame = {
        let mut rt = state.inner.lock().unwrap();
        if !rt.mascot.ready() || rt.mascot.pack_name != rt.settings.mascot_pack {
            // Wait out launch placement only when the window is actually
            // being placed from prefs (Collapsed restore); an app that
            // launches edge-hidden has no placement to wait for.
            if mode == PetMode::Collapsed && now_millis() < rt.suppress_until_ms {
                return;
            }
            let pack_name = rt.settings.mascot_pack.clone();
            let seed = now_millis();
            if !rt.mascot.activate(&pack_name, spawn, seed) {
                // Broken pack: fall back to the classic presentation instead
                // of an invisible pet.
                rt.settings.pet_kind = PetKind::Classic;
                rt.settings_store.save(&rt.settings);
                drop(rt);
                let view = {
                    let rt = state.inner.lock().unwrap();
                    state_view(&rt.state, &rt.settings)
                };
                let _ = app.emit("pet-state", view);
                return;
            }
        }
        rt.mascot.set_world(pet_engine::behavior::World {
            left: visible.x,
            right: visible.x + visible.w,
            top: visible.y,
            bottom: visible.y + visible.h,
        });
        if let Some(mood) = task_mood { rt.mascot.set_mood(mood); }
        let f = rt.mascot.tick();
        if std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok() && step >= 40 && step <= 45 {
            eprintln!("mascot-debug: tick({step}) -> {:?}", f);
        }
        f
    };
    let Some(frame) = frame else { return };
    let policy = state.inner.lock().unwrap().mascot.window_policy();
    // Only roaming pets move the window from the ticker. State pets use the
    // classic geometry pipeline (snap / edge hide / sliver), so the ticker
    // just hands the frame to the webview.
    if let (
        pet_engine::backend::PetFrame::Sprite { anchor, .. },
        pet_engine::backend::WindowPolicy::FollowsAnchor,
    ) = (&frame, policy)
    {
        // Move the window only while collapsed; in EdgeHidden the classic
        // sliver geometry owns the window and the frame must still reach the
        // webview so the pet peeks out of the strip.
        if mode == PetMode::Collapsed {
        let size_logical = state.inner.lock().unwrap().mascot.window_size(size_logical);
        let size_phys = size_logical * scale;
        let (x, y) = pet_engine::mascot::MascotHost::window_pos_for(
            *anchor,
            size_phys,
            MASCOT_BOTTOM_MARGIN * scale,
        );
        {
            let mut rt = state.inner.lock().unwrap();
            rt.suppress_until_ms = now_millis() + 120;
            rt.last_rect = Some(Rect::new(x as f64, y as f64, size_phys, size_phys));
        }
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
        let _ = win.set_size(tauri::PhysicalSize::new(
            size_phys.round() as u32,
            size_phys.round() as u32,
        ));
        }
    }
    {
        use std::sync::atomic::{AtomicU64 as AU64, Ordering as AOrd};
        static EMIT_LOGGED: AU64 = AU64::new(0);
        if std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok()
            && EMIT_LOGGED.fetch_add(1, AOrd::Relaxed) < 5
        {
            eprintln!("mascot-debug: emit pet-frame {:?}", frame);
        }
    }
    let _ = app.emit("pet-frame", &frame);
}

/// Global cursor position in logical screen points. Uses `CGEventGetLocation`,
/// which needs no accessibility permission (unlike event taps).
#[cfg(target_os = "macos")]
fn cursor_point() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let p = event.location();
    Some((p.x, p.y))
}

#[cfg(target_os = "windows")]
fn cursor_point() -> Option<(f64, f64)> {
    windows::cursor_position()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn cursor_point() -> Option<(f64, f64)> { None }

/// Expanded-panel dismissal fallback. The panel closes on window blur
/// ("click anywhere else"); if the blur never reaches the webview — focus
/// was never acquired, or the event was lost — an unfocused panel with the
/// cursor parked elsewhere is exactly "the user clicked somewhere else", so
/// collapse it too. Hover-expanded panels keep the pointer inside and still
/// close through the mouse-leave path, so they are untouched.
fn expanded_outside_dismissal(app: &AppHandle, win: &WebviewWindow, step: u64) {
    if step % 5 != 0 {
        return; // ≈200 ms cadence
    }
    let state = app.state::<AppState>();
    let mode = state.inner.lock().unwrap().state.mode;
    if mode != PetMode::Expanded {
        state.inner.lock().unwrap().expanded_dismiss_grace = 0;
        return;
    }
    let rect = tracked_rect(app, win);
    let (screens, idx) = screen_for(win, &rect);
    if screens.is_empty() {
        return;
    }
    let scale = screens[idx].scale;
    let focused = win.is_focused().unwrap_or(true);
    let debug = std::env::var("AGENT_MONITOR_DEBUG_MASCOT").is_ok();
    if focused {
        state.inner.lock().unwrap().expanded_dismiss_grace = 0;
        return;
    }
    let Some((cx, cy_up)) = cursor_point() else {
        return;
    };
    // CG reports a bottom-left origin; windows use top-left. Flip against
    // the virtual screen's lowest bottom edge.
    let max_bottom = screens
        .iter()
        .map(|s| s.visible.y + s.visible.h)
        .fold(0.0f64, f64::max);
    let cy = if cfg!(target_os = "macos") { max_bottom - cy_up } else { cy_up };
    // Window rects are physical px; the cursor is logical points.
    let unit_scale = if cfg!(target_os = "windows") { 1.0 } else { scale };
    let rx = rect.x / unit_scale;
    let ry = rect.y / unit_scale;
    let rw = rect.w / unit_scale;
    let rh = rect.h / unit_scale;
    let inside = cx >= rx && cx <= rx + rw && cy >= ry && cy <= ry + rh;
    if debug {
        eprintln!(
            "mascot-debug: expanded dismissal check focused={focused} cursor=({cx},{cy}) rect=({rx},{ry},{rw},{rh}) inside={inside}"
        );
    }
    let mut rt = state.inner.lock().unwrap();
    if inside {
        rt.expanded_dismiss_grace = 0;
        return;
    }
    rt.expanded_dismiss_grace += 1;
    let grace = rt.expanded_dismiss_grace;
    drop(rt);
    if grace >= 3 {
        // ≈600 ms of unfocused + cursor elsewhere: the user went somewhere
        // else; collapse whether or not the webview saw the blur.
        enter_mode(app, PetMode::Collapsed, false);
    }
}

fn handle_window_moved(app: AppHandle) {
    // Geometry written by us echoes back as Moved events (macOS applies
    // set_position/set_size asynchronously, so they can land outside the
    // suppression window). Reading those as user drags caused a thrash loop
    // (Dragging → settle → Collapsed → move → Moved …) that pinned the CPU,
    // and polluted the mascot's throw history with its own steps.
    let current = {
        let Some(win) = app.get_webview_window("main") else {
            return;
        };
        window_rect(&win)
    };
    {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        let suppressed = now_millis() < rt.suppress_until_ms;
        let own_echo = rt.last_rect.map(|last| {
            (current.x - last.x).abs() < 2.0
                && (current.y - last.y).abs() < 2.0
                && (current.w - last.w).abs() < 2.0
                && (current.h - last.h).abs() < 2.0
        });
        if suppressed || own_echo == Some(true) {
            return;
        }
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
        // Mascot throws need the recent window trajectory.
        if rt.settings.pet_kind == PetKind::Mascot {
            if let Some(rect) = rt.last_rect {
                rt.mascot.drag_history.push((now_millis(), rect.x, rect.y));
                let keep = rt.mascot.drag_history.len().saturating_sub(12);
                rt.mascot.drag_history.drain(..keep);
            }
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
        // Mascot release: no edge snapping — the engine takes the recorded
        // trajectory as a throw and the ticker re-places the window.
        let is_mascot = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            rt.settings.pet_kind == PetKind::Mascot && rt.mascot.ready()
        };
        // Only ROAMING pets bypass the classic settle (they throw from the
        // drag trajectory instead of snapping). State pets are the classic
        // pet with different artwork: they snap and persist like before.
        let is_roaming = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            rt.settings.pet_kind == PetKind::Mascot
                && rt.mascot.ready()
                && rt.mascot.window_policy() == pet_engine::backend::WindowPolicy::FollowsAnchor
        };
        if is_roaming {
            let size_phys = collapsed_size * scale;
            let anchor = pet_engine::mascot::MascotHost::anchor_of(
                (current.x, current.y, current.w, current.h),
                MASCOT_BOTTOM_MARGIN * scale,
            );
            {
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                rt.state.mode = PetMode::Collapsed;
                rt.state.edge = Edge::None;
                rt.state.hidden = false;
                rt.suppress_until_ms = now_millis() + 350;
                rt.mascot.drag_ended(anchor, now_millis());
                let view = state_view(&rt.state, &rt.settings);
                drop(rt);
                let _ = app.emit("pet-state", view);
            }
            let target = Rect::new(current.x, current.y, size_phys, size_phys);
            remember_rect(&app, target);
            set_rect(&win, target);
            persist(&app, target, Edge::None, PetMode::Collapsed);
            return;
        }
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
        let (tw, th) = target_size(mode, rows, collapsed_size, sliver, scale, snapped, edge);
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
        Some(task) => match serde_json::to_string_pretty(&jump::jump(task, &monitor.snapshot().tasks)) {
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
        focused_conversation: Option<ax::FocusedConversation>,
        terminal_tty: Option<String>,
        terminal_rollouts: Vec<PathBuf>,
        window_titles: Vec<String>,
        rows: Vec<ax::Row>,
        tree: String,
        sidebar: String,
    }
    let terminal_tty = jump::focused_terminal_tty();
    let terminal_rollouts = terminal_tty.as_deref().map(agents::codex::terminal_rollouts).unwrap_or_default();
    let dump = AxRowsDump {
        terminal_tty,
        terminal_rollouts,
        pid,
        trusted: ax::is_trusted(),
        focused_window_title: ax::focused_window_title(pid),
        focused_conversation: ax::focused_conversation(&["com.openai.codex".into(), "com.deepseek.dsh".into(), "com.microsoft.VSCode".into(), "dev.zcode.app".into()]),
        window_titles: ax::window_titles(pid),
        rows: ax::rows(pid),
        tree: ax::tree_dump(pid, 700),
        sidebar: ax::subtree_dump_by_id(pid, "sidebar", 700),
    };
    match serde_json::to_string_pretty(&dump) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("failed to serialise AX rows: {e}"),
    }
}

// ── Entry ───────────────────────────────────────────────────────

/// Debug-only recovery aid: with `AGENT_MONITOR_REQUEST_AX_PROMPT=1`, ask macOS
/// for the Accessibility prompt on behalf of THIS app shortly after startup. A
/// prompt triggered from the CLI would be attributed to the terminal, not the
/// packaged app, so it must run inside the app's own process. Never used by the
/// normal UI.
fn start_ax_prompt_request() {
    if std::env::var_os("AGENT_MONITOR_REQUEST_AX_PROMPT").is_none() {
        return;
    }
    std::thread::spawn(move || {
        // Let the app finish showing itself before the system prompt appears.
        std::thread::sleep(Duration::from_millis(2_000));
        match ax::request_accessibility_permission() {
            Ok(trusted) => eprintln!("ax-prompt: accessibility granted={trusted}"),
            Err(err) => eprintln!("ax-prompt: {err}"),
        }
    });
}

/// Debug/acceptance aid (never used by the normal UI): with
/// `AGENT_MONITOR_DUMP_JUMP_DIAGNOSTICS=<path>` set, write the same JSON as the
/// `get_jump_diagnostics` command to that path ~1.5 s after startup. This lets a
/// packaged `.app`'s real TCC identity/trust state be inspected without opening
/// Settings (debug-only; failures are logged and never affect the app).
fn start_jump_diagnostics_dump() {
    let Some(path) = std::env::var_os("AGENT_MONITOR_DUMP_JUMP_DIAGNOSTICS") else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1_500));
        let diagnostics = jump::collect_jump_diagnostics();
        let result = match serde_json::to_string_pretty(&diagnostics) {
            Ok(json) => std::fs::write(&path, json).map_err(|err| format!("write failed: {err}")),
            Err(err) => Err(format!("serialise failed: {err}")),
        };
        match result {
            Ok(()) => eprintln!(
                "jump-diagnostics: wrote {}",
                std::path::Path::new(&path).display()
            ),
            Err(err) => eprintln!("jump-diagnostics: {err}"),
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_pet(app);
        }))
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            ping,
            get_snapshot,
            watch_unread_completions,
            get_agents,
            set_task_pinned,
            set_task_name,
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
            request_accessibility_permission,
            reset_app_permissions,
            get_settings,
            set_settings,
            reset_settings,
            island_metrics,
            island_expand,
            island_collapse,
            set_island_mode,
            get_settings_meta,
            get_custom_icons,
            save_custom_icon,
            clear_custom_icon,
            list_pet_packs,
            pet_debug_log,
            get_pet_pack_image,
            import_pet_pack,
            set_mascot_action,
            pet_center,
            pet_menu_open,
            import_legacy_settings,
            open_settings,
            close_settings,
            open_data_dir,
            update::check_for_update,
            update::install_update,
            update::restart_app,
        ])
        .setup(|app| {
            #[cfg(target_os = "windows")]
            app.handle().plugin(tauri_plugin_notification::init())?;
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
                let screens = build_screens(&win);
                let idx = pick_screen(&screens, 0.0, 0.0);
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
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
                let (screens, idx) = screen_for(&win, &rect);
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
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

            // Island mode resumes at launch (macOS only; silently skipped
            // when no notched display is attached).
            #[cfg(target_os = "macos")]
            if initial_settings.island_mode {
                enter_island_mode(&handle, true);
            }

            // React to user drags and display changes.
            let app_for_events = handle.clone();
            win.on_window_event(move |event| match event {
                tauri::WindowEvent::Moved(_) => handle_window_moved(app_for_events.clone()),
                tauri::WindowEvent::ScaleFactorChanged { .. } => {
                    reconcile_screens(&app_for_events)
                }
                _ => {}
            });

            // Roaming mascot: one ticker drives the pet engine and both the
            // window position and the sprite frames. It self-pauses whenever
            // the pet is not in collapsed mascot mode.
            {
                let h = handle.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(
                        pet_engine::behavior::TICK_MS,
                    ));
                    mascot_ticker_step(&h);
                });
            }

            // Right-click action menu selections (native popup, see
            // pet_menu_open). Tray menu ids carry a different prefix.
            {
                let h = handle.clone();
                app.on_menu_event(move |app, event| {
                    let Some(action) = event.id().as_ref().strip_prefix("pet-action:") else {
                        return;
                    };
                    match action {
                        "__tasks" => {
                            enter_mode(app, PetMode::Expanded, false);
                            if let Some(win) = app.get_webview_window("main") { let _ = win.set_focus(); }
                            return;
                        }
                        "__keep_visible" => {
                            let mut next = settings_snapshot(app);
                            next.auto_hide_after_s = 0;
                            commit_settings(app, next);
                            show_pet(app);
                            return;
                        }
                        "__reset" => { reset_pet_position(app); return; }
                        "__settings" => { show_settings_window(app); return; }
                        "__hide" => { hide_pet_now(app); return; }
                        _ => {}
                    }
                    let state = app.state::<AppState>();
                    let mut rt = state.inner.lock().unwrap();
                    if action == "__center" {
                        rt.mascot.center();
                    } else {
                        rt.mascot.perform(action);
                    }
                });
            }

            // Headless diagnosis: surface webview script errors in stderr
            // (AGENT_MONITOR_DEBUG_MASCOT) — a crashed bootstrap leaves the
            // pet window blank with no other trace.
            {
                let h = handle.clone();
                let _ = win.eval(
                    "window.addEventListener('error', function(e){ try { __TAURI__.core.invoke('pet_debug_log', { msg: 'JSERR ' + e.message + ' @' + e.filename + ':' + e.lineno }); } catch (_) {} }); window.addEventListener('unhandledrejection', function(e){ try { __TAURI__.core.invoke('pet_debug_log', { msg: 'JSREJ ' + e.reason }); } catch (_) {} });",
                );
                let _ = h;
            }

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

            // In-app updater: one silent background check ~4 s after launch
            // (`auto_check_updates`), or the debug-only auto install path used
            // by the acceptance run (docs/UPDATING.md). Both never block setup.
            update::spawn_startup_check(&handle);
            update::spawn_auto_install_if_requested(&handle);

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
            start_ax_prompt_request();
            start_jump_diagnostics_dump();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── Unit tests (pure helpers only; Tauri plumbing needs a real app) ──

#[cfg(test)]
mod tests {
    use super::*;

    fn completed_task(id: &str, title: &str) -> model::TaskRecord {
        model::TaskRecord {
            id: format!("codex:{id}"), agent_key: "codex".into(), agent_name: "Codex".into(),
            host: "desktop".into(), session_id: Some(id.into()), task_id: None,
            title: title.into(), status: model::TaskStatus::Completed,
            completion_id: Some("100".into()),
            completion: Some(model::CompletionEvidence::TaskEnd), status_reason: "task_complete".into(),
            last_activity_secs: 100, collected_at_secs: 100, summary: None, model: None,
            origin: model::DataOrigin::Event,
            target: model::JumpTarget { app_name: Some("ChatGPT".into()), bundle_id: Some("com.openai.codex".into()), ..Default::default() },
            target_validity: model::TargetValidity::Valid, invalid_reason: None,
            capability: model::JumpCapability::default(), level: model::JumpLevel::L3, pinned: false,
        }
    }

    #[test]
    fn terminal_view_matches_the_open_rollout_even_with_duplicate_titles() {
        let mut first = completed_task("one", "Same title"); first.host = "terminal".into();
        let mut second = completed_task("two", "Same title"); second.host = "terminal".into();
        let tasks = vec![first, second];
        let unread = vec![ViewedCompletion { task_id: "codex:one".into(), completion_id: "100".into() },
            ViewedCompletion { task_id: "codex:two".into(), completion_id: "100".into() }];
        let paths = vec![PathBuf::from("/home/sessions/2026/11/03/rollout-2026-11-03-one.jsonl")];
        assert_eq!(terminal_viewed_completion(&paths, &tasks, &unread), Some(unread[0].clone()));
        let other = vec![PathBuf::from("/home/sessions/rollout-date-one-longer.jsonl")];
        assert_eq!(terminal_viewed_completion(&other, &tasks, &unread), None);
        let ambiguous = vec![paths[0].clone(), PathBuf::from("/home/sessions/rollout-date-two.jsonl")];
        assert_eq!(terminal_viewed_completion(&ambiguous, &tasks, &unread), None);
    }

    #[test]
    fn terminal_view_does_not_read_running_tasks_or_newer_answers() {
        let mut task = completed_task("one", "Task"); task.host = "terminal".into();
        let paths = vec![PathBuf::from("/home/sessions/rollout-date-one.jsonl")];
        let old = vec![ViewedCompletion { task_id: task.id.clone(), completion_id: "99".into() }];
        assert_eq!(terminal_viewed_completion(&paths, &[task.clone()], &old), None);
        let current = vec![ViewedCompletion { task_id: task.id.clone(), completion_id: "100".into() }];
        task.status = model::TaskStatus::Running;
        assert_eq!(terminal_viewed_completion(&paths, &[task.clone()], &current), None);
        task.status = model::TaskStatus::Completed;
        task.host = "desktop".into();
        assert_eq!(terminal_viewed_completion(&paths, &[task], &current), None);
    }

    #[test]
    fn manual_view_requires_the_exact_conversation_in_the_foreground_app() {
        let tasks = vec![completed_task("one", "First task"), completed_task("two", "Second task")];
        let unread = vec![ViewedCompletion { task_id: "codex:one".into(), completion_id: "100".into() }];
        let mut context = ax::FocusedConversation { app_name: "ChatGPT".into(), bundle_id: "com.openai.codex".into(), title: "First task — ChatGPT".into(), ..Default::default() };
        assert_eq!(viewed_completion(&context, &tasks, &unread), Some(unread[0].clone()));
        context.app_name = "Renamed Codex".into();
        context.title = "First task — Renamed Codex".into();
        assert_eq!(viewed_completion(&context, &tasks, &unread), Some(unread[0].clone()));
        context.app_name = "ChatGPT".into();
        context.title = "First".into();
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
        context.title = "First task".into();
        context.bundle_id = "other.app".into();
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
        context.bundle_id = "com.openai.codex".into();
        context.title = "Some workspace".into();
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
        context.selected_titles.push("First task".into());
        assert_eq!(viewed_completion(&context, &tasks, &unread), Some(unread[0].clone()));
    }

    #[test]
    fn duplicate_titles_require_a_session_identifier_even_if_only_one_is_unread() {
        let tasks = vec![completed_task("one", "Same title"), completed_task("two", "Same title")];
        let unread = vec![ViewedCompletion { task_id: "codex:one".into(), completion_id: "100".into() }];
        let mut context = ax::FocusedConversation { app_name: "ChatGPT".into(), bundle_id: "com.openai.codex".into(), title: "Same title".into(), ..Default::default() };
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
        context.references.push("codex://threads/one-longer".into());
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
        context.references = vec!["codex://threads/one".into()];
        assert_eq!(viewed_completion(&context, &tasks, &unread), Some(unread[0].clone()));
    }

    #[test]
    fn manual_view_does_not_acknowledge_running_or_a_different_completion_version() {
        let mut tasks = vec![completed_task("one", "Task")];
        let unread = vec![ViewedCompletion { task_id: "codex:one".into(), completion_id: "99".into() }];
        let context = ax::FocusedConversation { app_name: "ChatGPT".into(), bundle_id: "com.openai.codex".into(), title: "Task".into(), ..Default::default() };
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
        tasks[0].completion_id = Some("99".into());
        tasks[0].last_activity_secs = 999;
        assert_eq!(viewed_completion(&context, &tasks, &unread), Some(unread[0].clone()));
        tasks[0].last_activity_secs = 99;
        tasks[0].status = model::TaskStatus::Running;
        assert_eq!(viewed_completion(&context, &tasks, &unread), None);
    }

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
