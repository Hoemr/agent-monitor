//! Agent Monitor — desktop-pet task monitor for multiple coding agents.
//!
//! Responsibilities are split into modules:
//!   * `agents/*`  — one adapter per real local data source
//!   * `monitor`   — collects adapters into one deduplicated snapshot
//!   * `jump`      — graded, verified jump execution (L3→L0)
//!   * `pet`       — pure geometry + placement persistence
//!   * this file   — Tauri window/tray/command plumbing only

pub mod agents;
pub mod jump;
pub mod model;
pub mod monitor;
pub mod pet;

use model::{
    now_millis, AgentDescriptor, Edge, JumpLevel, JumpOutcome, PermissionKind, PetMode, PetPrefs,
    PetState, Snapshot,
};
use monitor::Monitor;
use pet::{
    anchored_resize, constrain, expanded_height_logical, finish_drag_with, hide_rect, nearest_edge,
    pick_screen, position_from_rect, restore_rect, state_view, unhide_rect, PetStore, Rect,
    ScreenRect, COLLAPSED_SIZE, EXPANDED_WIDTH, HIDDEN_SLIVER, SNAP_DISTANCE,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow};

static ANIMATION_GENERATION: AtomicU64 = AtomicU64::new(0);
static DRAG_GENERATION: AtomicU64 = AtomicU64::new(0);

// ── Runtime state ───────────────────────────────────────────────

struct Runtime {
    monitor: Monitor,
    prefs: PetPrefs,
    store: PetStore,
    state: PetState,
    /// Window moves caused by us must not be mistaken for user drags.
    suppress_until_ms: u64,
    mode_before_drag: PetMode,
    /// Authoritative window rectangle: macOS applies set_position
    /// asynchronously, so reading it back immediately after a move is stale.
    last_rect: Option<Rect>,
}

pub struct AppState {
    inner: Mutex<Runtime>,
}

impl AppState {
    fn new() -> Self {
        let store = PetStore::new(PetStore::default_path());
        let prefs = store.load();
        Self {
            inner: Mutex::new(Runtime {
                monitor: Monitor::new(),
                state: PetState {
                    hidden: prefs.hidden,
                    ..Default::default()
                },
                prefs,
                store,
                suppress_until_ms: 0,
                mode_before_drag: PetMode::Collapsed,
                last_rect: None,
            }),
        }
    }
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

fn target_size(mode: PetMode, rows: usize, scale: f64, current: Rect) -> (f64, f64) {
    match mode {
        PetMode::Expanded => (
            EXPANDED_WIDTH * scale,
            expanded_height_logical(rows) * scale,
        ),
        PetMode::Collapsed | PetMode::EdgeHidden => {
            (COLLAPSED_SIZE * scale, COLLAPSED_SIZE * scale)
        }
        PetMode::Dragging => (current.w, current.h),
    }
}

fn animate(win: &WebviewWindow, from: Rect, to: Rect) {
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
    rt.prefs.hidden = mode == PetMode::EdgeHidden;
    rt.state.edge = edge;
    rt.state.hidden = mode == PetMode::EdgeHidden;
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
    let (rows, edge, was_mode) = {
        let rt = state.inner.lock().unwrap();
        (
            rt.monitor.snapshot().tasks.len(),
            rt.state.edge,
            rt.state.mode,
        )
    };
    let edge = if edge == Edge::None {
        nearest_edge(&current, &visible)
    } else {
        edge
    };
    let (tw, th) = target_size(mode, rows, scale, current);
    let base = Rect::new(current.x, current.y, tw, th);
    let target = match mode {
        PetMode::EdgeHidden => hide_rect(constrain(base, &visible), edge, HIDDEN_SLIVER * scale),
        PetMode::Expanded => {
            if was_mode == PetMode::EdgeHidden {
                unhide_rect(current, edge, (tw, th), &visible)
            } else {
                anchored_resize(base, edge, (tw, th), &visible)
            }
        }
        PetMode::Collapsed => {
            let base = if was_mode == PetMode::EdgeHidden {
                unhide_rect(current, edge, (tw, th), &visible)
            } else {
                current
            };
            anchored_resize(base, edge, (tw, th), &visible)
        }
        PetMode::Dragging => current,
    };

    {
        let mut rt = state.inner.lock().unwrap();
        rt.state.mode = mode;
        rt.state.edge = edge;
        rt.state.hidden = mode == PetMode::EdgeHidden;
        rt.suppress_until_ms = now_millis() + 420;
        let (screens, idx) = screen_for(&win, &target);
        rt.prefs.position = position_from_rect(&target, &screens[idx], edge);
        rt.prefs.hidden = rt.state.hidden;
        let view = state_view(&rt.state, &rt.prefs);
        rt.store.save(&rt.prefs);
        drop(rt);
        let _ = app.emit("pet-state", view);
    }

    remember_rect(app, target);
    if animated {
        animate(&win, current, target);
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
    let (mode, edge) = {
        let state = app.state::<AppState>();
        let rt = state.inner.lock().unwrap();
        (rt.state.mode, rt.state.edge)
    };
    if mode == PetMode::Dragging {
        return;
    }
    let target = if mode == PetMode::EdgeHidden && edge != Edge::None {
        hide_rect(
            constrain(current, &visible),
            edge,
            HIDDEN_SLIVER * screens[idx].scale,
        )
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
        let _ = win.show();
        let state = app.state::<AppState>();
        let suppress = {
            let rt = state.inner.lock().unwrap();
            (rt.state.mode, now_millis() < rt.suppress_until_ms)
        };
        if suppress.0 == PetMode::EdgeHidden || suppress.0 == PetMode::Collapsed {
            enter_mode(app, PetMode::Collapsed, false);
        }
    }
}

fn hide_pet_to_edge(app: &AppHandle) {
    enter_mode(app, PetMode::EdgeHidden, true);
}

fn reset_pet_position(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let screens = build_screens(&win);
    let idx = pick_screen(&screens, 0.0, 0.0);
    let s = &screens[idx];
    let size = COLLAPSED_SIZE * s.scale;
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
        rt.monitor.scan(true);
        rt.monitor.snapshot().clone()
    };
    let _ = app.emit("tasks-updated", &snapshot);
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
    }
    ok
}

#[tauri::command]
async fn jump_to_task(
    app: AppHandle,
    task_id: String,
) -> Result<JumpOutcome, String> {
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
    state_view(&rt.state, &rt.prefs)
}

#[tauri::command]
fn set_expanded(app: AppHandle, state: State<'_, AppState>, expanded: bool) {
    let mode = { state.inner.lock().unwrap().state.mode };
    if expanded {
        if mode != PetMode::Expanded {
            enter_mode(&app, PetMode::Expanded, true);
        }
    } else if mode == PetMode::Expanded {
        enter_mode(&app, PetMode::Collapsed, true);
    }
}

#[tauri::command]
fn request_collapse(app: AppHandle, state: State<'_, AppState>) {
    let (mode, suppressed) = {
        let rt = state.inner.lock().unwrap();
        (rt.state.mode, now_millis() < rt.suppress_until_ms)
    };
    if mode == PetMode::Expanded && !suppressed {
        enter_mode(&app, PetMode::Collapsed, true);
    }
}

#[tauri::command]
fn hide_pet(app: AppHandle) {
    hide_pet_to_edge(&app);
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

fn start_worker(app: AppHandle) {
    let (tx, rx) = channel::<()>();
    start_watcher(tx);
    std::thread::spawn(move || loop {
        let (interval, hidden, empty) = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            let hidden = rt.state.mode == PetMode::EdgeHidden || rt.state.hidden;
            let empty = rt.monitor.snapshot().tasks.is_empty();
            (if hidden || empty { 30 } else { 5 }, hidden, empty)
        };
        let _ = rx.recv_timeout(Duration::from_secs(interval));
        while rx.try_recv().is_ok() {}

        let changed = {
            let state = app.state::<AppState>();
            let mut rt = state.inner.lock().unwrap();
            rt.monitor.scan(false)
        };
        if changed {
            let snapshot = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.monitor.snapshot().clone()
            };
            if !hidden {
                let _ = app.emit("tasks-updated", &snapshot);
            }
            update_tray_tooltip(&app, &snapshot);
        }
        if !(hidden && empty) {
            reconcile_screens(&app);
        }
    });
}

fn update_tray_tooltip(app: &AppHandle, snapshot: &Snapshot) {
    let waiting = snapshot.tasks.iter().filter(|t| t.needs_attention()).count();
    let total = snapshot.tasks.len();
    let label = if total == 0 {
        "Agent Monitor — no sessions".to_string()
    } else if waiting > 0 {
        format!(
            "Agent Monitor — {} session(s), {} need attention",
            total, waiting
        )
    } else {
        format!("Agent Monitor — {} session(s)", total)
    };
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(&label));
    }
}

// ── Tray ────────────────────────────────────────────────────────

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::TrayIconEvent;

    let show_item = MenuItemBuilder::with_id("show", "Show Pet").build(app)?;
    let hide_item = MenuItemBuilder::with_id("hide", "Hide to Edge").build(app)?;
    let reset_item = MenuItemBuilder::with_id("reset", "Reset Position").build(app)?;
    let refresh_item = MenuItemBuilder::with_id("refresh", "Refresh").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&hide_item)
        .item(&reset_item)
        .separator()
        .item(&refresh_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let tray = app
        .tray_by_id("main-tray")
        .ok_or("tray icon missing; check tauri.conf.json")?;
    tray.set_menu(Some(menu))?;
    tray.set_tooltip(Some("Agent Monitor"))?;

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
        "show" => show_pet(app),
        "hide" => hide_pet_to_edge(app),
        "reset" => reset_pet_position(app),
        "refresh" => {
            let snapshot = {
                let state = app.state::<AppState>();
                let mut rt = state.inner.lock().unwrap();
                rt.monitor.scan(true);
                rt.monitor.snapshot().clone()
            };
            let _ = app.emit("tasks-updated", &snapshot);
            update_tray_tooltip(app, &snapshot);
        }
        "quit" => {
            {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                rt.store.save(&rt.prefs);
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
        let (snapped, edge) = finish_drag_with(current, &visible, SNAP_DISTANCE * scale);
        let (mode_before, rows) = {
            let state = app.state::<AppState>();
            let rt = state.inner.lock().unwrap();
            (rt.mode_before_drag, rt.monitor.snapshot().tasks.len())
        };
        let mode = match mode_before {
            PetMode::Expanded => PetMode::Expanded,
            _ => PetMode::Collapsed,
        };
        let (tw, th) = target_size(mode, rows, scale, snapped);
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
            let view = state_view(&rt.state, &rt.prefs);
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
/// starting the GUI. Used by the acceptance checklist and docs evidence.
pub fn dump_tasks() {
    let mut monitor = Monitor::new();
    monitor.scan(true);
    let first: Vec<String> = monitor.snapshot().tasks.iter().map(|t| t.id.clone()).collect();
    monitor.scan(true);
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
    let mut monitor = Monitor::new();
    monitor.scan(true);
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
    let monitor = Monitor::new();
    match serde_json::to_string_pretty(&monitor.descriptors()) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("failed to serialise agents: {e}"),
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
            jump_to_task,
            get_pet_state,
            set_expanded,
            request_collapse,
            hide_pet,
            show_pet_command,
            reset_pet_position_command,
            open_permission_settings,
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

            // Initial position from persisted prefs, constrained to a live screen.
            let (screens, idx, prefs, hidden) = {
                let state = app.state::<AppState>();
                let rt = state.inner.lock().unwrap();
                let screens = build_screens(&win);
                let idx = pick_screen(&screens, 0.0, 0.0);
                (screens, idx, rt.prefs.clone(), rt.prefs.hidden)
            };
            let scale = screens[idx].scale;
            let size = COLLAPSED_SIZE * scale;
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
                enter_mode(&handle, PetMode::EdgeHidden, false);
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
                rt.monitor.scan(true);
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
                Ok("collapsed") => enter_mode(&handle, PetMode::Collapsed, false),
                _ => {}
            }
            start_worker(handle.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
