//! Desktop-pet geometry, persistence and state model.
//!
//! All geometry math is pure and unit-tested (negative coordinates, stacked
//! displays, scale changes, screen removal). Window plumbing lives in
//! `lib.rs`; this module never talks to Tauri.

use crate::model::{Edge, PetMode, PetPosition, PetPrefs, PetState};
use crate::settings::{PetKind, Settings};
use serde::Serialize;
use std::path::PathBuf;

// Logical (point) design constants. The caller multiplies by the target
// screen's scale factor before touching the window.
pub const COLLAPSED_SIZE: f64 = 72.0;
pub const EXPANDED_WIDTH: f64 = 460.0;
pub const HEADER_HEIGHT: f64 = 44.0;
pub const ROW_HEIGHT: f64 = 64.0;
pub const FOOTER_HEIGHT: f64 = 26.0;
pub const MAX_VISIBLE_ROWS: usize = 8;
pub const SNAP_DISTANCE: f64 = 16.0;
pub const HIDDEN_SLIVER: f64 = 14.0;
pub const SCREEN_MARGIN: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn intersection_area(&self, other: &Rect) -> f64 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScreenRect {
    pub key: String,
    pub frame: Rect,
    pub visible: Rect,
    pub scale: f64,
}

/// Expanded window height for `rows` visible rows. The caller passes the row
/// count already capped by the user setting (`Settings::visible_rows`); this is
/// the last line of defence and supports the full 1..=8 range.
pub fn expanded_height_logical(rows: usize) -> f64 {
    let rows = rows.clamp(1, MAX_VISIBLE_ROWS);
    HEADER_HEIGHT + ROW_HEIGHT * rows as f64 + FOOTER_HEIGHT
}

/// Which screen should host a point. Falls back to the largest overlap and
/// finally to the first screen (the host always provides at least one).
pub fn pick_screen(screens: &[ScreenRect], x: f64, y: f64) -> usize {
    if screens.is_empty() {
        return 0;
    }
    if let Some(i) = screens.iter().position(|s| s.frame.contains(x, y)) {
        return i;
    }
    let probe = Rect::new(x, y, 1.0, 1.0);
    let mut best = 0;
    let mut best_area = 0.0;
    for (i, s) in screens.iter().enumerate() {
        let area = probe.intersection_area(&s.frame);
        if area > best_area {
            best_area = area;
            best = i;
        }
    }
    best
}

/// Restore a saved position inside a screen's visible frame.
///
/// `rel_x`/`rel_y` are fractions of the *available* range so a resolution or
/// Dock change keeps the pet in a comparable spot.
pub fn restore_rect(pos: &PetPosition, size: (f64, f64), screens: &[ScreenRect]) -> Rect {
    let idx = screens
        .iter()
        .position(|s| s.key == pos.screen_key)
        .unwrap_or_else(|| {
            // Screen gone (cable unplugged / rearranged): use the screen that
            // contains the primary visible point, else the first one.
            pick_screen(screens, 0.0, 0.0)
        });
    let s = screens.get(idx).cloned().unwrap_or(ScreenRect {
        key: String::new(),
        frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        visible: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        scale: 1.0,
    });
    let v = s.visible;
    let max_x = (v.w - size.0).max(0.0);
    let max_y = (v.h - size.1).max(0.0);
    let x = v.x + max_x * pos.rel_x.clamp(0.0, 1.0);
    let y = v.y + max_y * pos.rel_y.clamp(0.0, 1.0);
    constrain(Rect::new(x, y, size.0, size.1), &v)
}

/// Persist a rectangle as fractions of the current screen's visible frame.
pub fn position_from_rect(rect: &Rect, screen: &ScreenRect, edge: Edge) -> PetPosition {
    let v = screen.visible;
    let max_x = (v.w - rect.w).max(1.0);
    let max_y = (v.h - rect.h).max(1.0);
    PetPosition {
        screen_key: screen.key.clone(),
        rel_x: ((rect.x - v.x) / max_x).clamp(0.0, 1.0),
        rel_y: ((rect.y - v.y) / max_y).clamp(0.0, 1.0),
        edge,
    }
}

/// Clamp a rectangle fully inside a visible frame. When the frame is smaller
/// than the window, the window is pinned to the frame origin (never off-screen).
pub fn constrain(rect: Rect, visible: &Rect) -> Rect {
    let max_x = (visible.right() - rect.w).max(visible.x);
    let max_y = (visible.bottom() - rect.h).max(visible.y);
    Rect {
        x: rect.x.clamp(visible.x, max_x),
        y: rect.y.clamp(visible.y, max_y),
        w: rect.w,
        h: rect.h,
    }
}

/// Distance from the rectangle to each edge of the visible frame.
fn edge_distances(rect: &Rect, visible: &Rect) -> [(Edge, f64); 4] {
    [
        (Edge::Left, (rect.x - visible.x).abs()),
        (Edge::Right, (visible.right() - rect.right()).abs()),
        (Edge::Top, (rect.y - visible.y).abs()),
        (Edge::Bottom, (visible.bottom() - rect.bottom()).abs()),
    ]
}

/// Decide the edge to snap to. Deterministic at corners: the nearest edge
/// wins; ties resolve Left > Right > Top > Bottom by declaration order.
pub fn snap_edge(rect: &Rect, visible: &Rect) -> Option<Edge> {
    snap_edge_with(rect, visible, SNAP_DISTANCE)
}

/// Same as [`snap_edge`] with an explicit distance (physical pixels).
pub fn snap_edge_with(rect: &Rect, visible: &Rect, distance: f64) -> Option<Edge> {
    let mut best: Option<(Edge, f64)> = None;
    for (edge, dist) in edge_distances(rect, visible) {
        if dist <= distance && best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((edge, dist));
        }
    }
    best.map(|(e, _)| e)
}

/// Nearest edge regardless of distance (used when the user asks to hide the
/// pet while it sits in a free position).
pub fn nearest_edge(rect: &Rect, visible: &Rect) -> Edge {
    let mut best = Edge::Right;
    let mut best_d = f64::MAX;
    for (edge, dist) in edge_distances(rect, visible) {
        if dist < best_d {
            best_d = dist;
            best = edge;
        }
    }
    best
}

/// Snap and clamp after a drag. Snapping happens only at drag end.
pub fn finish_drag(rect: Rect, visible: &Rect) -> (Rect, Edge) {
    finish_drag_with(rect, visible, SNAP_DISTANCE)
}

/// Same as [`finish_drag`] with an explicit snap distance (physical pixels).
pub fn finish_drag_with(rect: Rect, visible: &Rect, distance: f64) -> (Rect, Edge) {
    let clamped = constrain(rect, visible);
    match snap_edge_with(&clamped, visible, distance) {
        Some(edge) => {
            let mut r = clamped;
            match edge {
                Edge::Left => r.x = visible.x,
                Edge::Right => r.x = visible.right() - r.w,
                Edge::Top => r.y = visible.y,
                Edge::Bottom => r.y = visible.bottom() - r.h,
                Edge::None => {}
            }
            (constrain(r, visible), edge)
        }
        None => (clamped, Edge::None),
    }
}

/// Hide as a narrow strip; its along-edge length follows collapsed_size.
pub fn hide_tab_rect(rect: Rect, visible: &Rect, edge: Edge, sliver: f64, collapsed_size: f64) -> Rect {
    let short = sliver.max(1.0);
    let long = collapsed_size.max(1.0);
    let (w, h) = match edge {
        Edge::Top | Edge::Bottom => (long, short),
        _ => (short, long),
    };
    let (cx, cy) = rect.center();
    let along_y = (cy - h / 2.0).clamp(visible.y, (visible.bottom() - h).max(visible.y));
    let along_x = (cx - w / 2.0).clamp(visible.x, (visible.right() - w).max(visible.x));
    match edge {
        Edge::Left => Rect::new(visible.x, along_y, w, h),
        Edge::Right => Rect::new(visible.right() - w, along_y, w, h),
        Edge::Top => Rect::new(along_x, visible.y, w, h),
        Edge::Bottom => Rect::new(along_x, visible.bottom() - h, w, h),
        Edge::None => constrain(rect, visible),
    }
}

/// Expand from the hidden tab back on screen, anchored to its edge and keeping
/// the tab's position along that edge.
pub fn unhide_rect(hidden: Rect, edge: Edge, size: (f64, f64), visible: &Rect) -> Rect {
    let (hcx, hcy) = hidden.center();
    let r = match edge {
        Edge::Left => Rect::new(visible.x, hcy - size.1 / 2.0, size.0, size.1),
        Edge::Right => Rect::new(visible.right() - size.0, hcy - size.1 / 2.0, size.0, size.1),
        Edge::Top => Rect::new(hcx - size.0 / 2.0, visible.y, size.0, size.1),
        Edge::Bottom => Rect::new(hcx - size.0 / 2.0, visible.bottom() - size.1, size.0, size.1),
        Edge::None => Rect::new(hidden.x, hidden.y, size.0, size.1),
    };
    constrain(r, visible)
}

/// Expand/collapse keeping the anchored edge fixed and growing inward.
pub fn anchored_resize(rect: Rect, edge: Edge, size: (f64, f64), visible: &Rect) -> Rect {
    let mut r = Rect::new(rect.x, rect.y, size.0, size.1);
    match edge {
        Edge::Right => r.x = rect.right() - size.0,
        Edge::Left => r.x = rect.x,
        Edge::Top => r.y = rect.y,
        Edge::Bottom => r.y = rect.bottom() - size.1,
        Edge::None => {
            // Free position: keep the top-left corner.
        }
    }
    constrain(r, visible)
}

/// Snapshot of everything the pet webview needs to render its state. The
/// embedded [`Settings`] removes any chance of the UI and Rust disagreeing
/// (language, row cap, animations, toggles…).
#[derive(Clone, Debug, Serialize)]
pub struct PetStateView {
    pub mode: &'static str,
    pub edge: &'static str,
    pub hidden: bool,
    /// True when the window is ordered out (tray-hidden) instead of parked at
    /// the screen edge.
    pub hidden_to_tray: bool,
    /// True when the collapsed pet is a roaming mascot (docs/PETS.md); the
    /// frontend then renders the `pet-frame` sprite instead of the capsule.
    pub mascot: bool,
    pub settings: Settings,
}

pub fn mode_key(mode: PetMode) -> &'static str {
    match mode {
        PetMode::Collapsed => "collapsed",
        PetMode::Expanded => "expanded",
        PetMode::Dragging => "dragging",
        PetMode::EdgeHidden => "edge_hidden",
        PetMode::TrayHidden => "tray_hidden",
    }
}

pub fn edge_key(edge: Edge) -> &'static str {
    match edge {
        Edge::Left => "left",
        Edge::Right => "right",
        Edge::Top => "top",
        Edge::Bottom => "bottom",
        Edge::None => "none",
    }
}

pub fn state_view(state: &PetState, settings: &Settings) -> PetStateView {
    PetStateView {
        mode: mode_key(state.mode),
        edge: edge_key(state.edge),
        hidden: state.hidden,
        hidden_to_tray: state.mode == PetMode::TrayHidden,
        mascot: settings.pet_kind == PetKind::Mascot,
        settings: settings.clone(),
    }
}

// ── Persistence ─────────────────────────────────────────────────

pub struct PetStore {
    path: PathBuf,
}

impl PetStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        crate::settings::pet_path()
    }

    pub fn load(&self) -> PetPrefs {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best-effort, atomic-enough write; a failure never blocks the UI.
    pub fn save(&self, prefs: &PetPrefs) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(prefs) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(key: &str, x: f64, y: f64, w: f64, h: f64) -> ScreenRect {
        ScreenRect {
            key: key.into(),
            frame: Rect::new(x, y, w, h),
            visible: Rect::new(x, y, w, h),
            scale: 2.0,
        }
    }

    #[test]
    fn snap_picks_nearest_edge_only() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let near_right = Rect::new(1838.0, 500.0, 72.0, 72.0); // right gap 10
        let (r, edge) = finish_drag(near_right, &vis);
        assert_eq!(edge, Edge::Right);
        assert_eq!(r.right(), 1920.0);
        assert_eq!(r.x, 1848.0);
    }

    #[test]
    fn corner_chooses_nearest_edge_deterministically() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        // Closer to the bottom (4) than to the right (12).
        let r = Rect::new(1836.0, 1004.0, 72.0, 72.0);
        let (snapped, edge) = finish_drag(r, &vis);
        assert_eq!(edge, Edge::Bottom);
        assert_eq!(snapped.bottom(), 1080.0);
    }

    #[test]
    fn no_snap_when_far_from_edges() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let r = Rect::new(800.0, 500.0, 72.0, 72.0);
        let (out, edge) = finish_drag(r, &vis);
        assert_eq!(edge, Edge::None);
        assert_eq!(out, r);
    }

    #[test]
    fn drag_never_leaves_visible_frame() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let r = Rect::new(-500.0, 2000.0, 72.0, 72.0);
        let (out, _) = finish_drag(r, &vis);
        assert_eq!(out.x, 0.0);
        assert_eq!(out.bottom(), 1080.0);
    }

    #[test]
    fn hidden_strip_matches_collapsed_size_and_stays_flush_with_each_edge() {
        let visible = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let rect = Rect::new(1500.0, 400.0, 360.0, 400.0);
        for size in [48.0, 56.0, 72.0, 96.0] {
            for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                let hidden = hide_tab_rect(rect, &visible, edge, HIDDEN_SLIVER, size);
                let expected = if matches!(edge, Edge::Left | Edge::Right) { (HIDDEN_SLIVER, size) } else { (size, HIDDEN_SLIVER) };
                assert_eq!((hidden.w, hidden.h), expected);
                assert_eq!(constrain(hidden, &visible), hidden);
                match edge {
                    Edge::Left => assert_eq!(hidden.x, visible.x),
                    Edge::Right => assert_eq!(hidden.right(), visible.right()),
                    Edge::Top => assert_eq!(hidden.y, visible.y),
                    Edge::Bottom => assert_eq!(hidden.bottom(), visible.bottom()),
                    Edge::None => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn hidden_tab_keeps_the_position_along_the_edge() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let r = Rect::new(1848.0, 500.0, 72.0, 72.0);
        let (_, cy) = r.center();
        let h = hide_tab_rect(r, &vis, Edge::Right, HIDDEN_SLIVER, COLLAPSED_SIZE);
        assert!((h.center().1 - cy).abs() < 0.001);
        // And it never leaves the visible frame.
        assert!(h.y >= vis.y && h.bottom() <= vis.bottom());
    }

    #[test]
    fn unhide_returns_fully_on_screen_and_keeps_the_along_edge_center() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let expanded = (360.0, 400.0);
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let hidden = hide_tab_rect(Rect::new(1500.0, 400.0, 360.0, 400.0), &vis, edge, HIDDEN_SLIVER, COLLAPSED_SIZE);
            let out = unhide_rect(hidden, edge, expanded, &vis);
            assert!(out.x >= vis.x - 0.001 && out.right() <= vis.right() + 0.001);
            assert!(out.y >= vis.y - 0.001 && out.bottom() <= vis.bottom() + 0.001);
            let along = match edge {
                Edge::Left | Edge::Right => (out.center().1 - hidden.center().1).abs(),
                Edge::Top | Edge::Bottom => (out.center().0 - hidden.center().0).abs(),
                Edge::None => 0.0,
            };
            assert!(along < 0.001, "edge {:?} moved along the edge by {}", edge, along);
        }
    }

    #[test]
    fn expanding_at_right_edge_grows_leftwards() {
        let vis = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let collapsed = Rect::new(1848.0, 500.0, 72.0, 72.0);
        let out = anchored_resize(collapsed, Edge::Right, (360.0, 420.0), &vis);
        assert_eq!(out.right(), collapsed.right());
        assert_eq!(out.x, 1848.0 + 72.0 - 360.0);
    }

    #[test]
    fn expanded_height_is_bounded_by_rows() {
        assert_eq!(expanded_height_logical(1), HEADER_HEIGHT + ROW_HEIGHT + FOOTER_HEIGHT);
        assert_eq!(expanded_height_logical(5), HEADER_HEIGHT + ROW_HEIGHT * 5.0 + FOOTER_HEIGHT);
        assert_eq!(expanded_height_logical(8), HEADER_HEIGHT + ROW_HEIGHT * 8.0 + FOOTER_HEIGHT);
        assert_eq!(expanded_height_logical(40), expanded_height_logical(8));
        assert_eq!(expanded_height_logical(0), expanded_height_logical(1));
    }

    #[test]
    fn row_presets_3_5_8_produce_exact_native_heights() {
        for (rows, expected) in [
            (3usize, 44.0 + 64.0 * 3.0 + 26.0),
            (5usize, 44.0 + 64.0 * 5.0 + 26.0),
            (8usize, 44.0 + 64.0 * 8.0 + 26.0),
        ] {
            assert_eq!(expanded_height_logical(rows), expected);
        }
        // 8 rows still fits an 1080 pt visible frame (862 + menu bar margins).
        assert!(expanded_height_logical(8) < 1080.0);
    }

    #[test]
    fn state_view_carries_mode_and_full_settings() {
        let state = PetState {
            mode: PetMode::TrayHidden,
            edge: Edge::Right,
            hidden: true,
        };
        let mut settings = Settings::default();
        settings.max_visible_rows = 8;
        settings.hover_expand_ms = 300;
        let view = state_view(&state, &settings);
        assert_eq!(view.mode, "tray_hidden");
        assert!(view.hidden);
        assert!(view.hidden_to_tray);
        assert_eq!(view.settings.max_visible_rows, 8);
        assert_eq!(view.settings.hover_expand_ms, 300);

        let visible = PetState {
            mode: PetMode::Collapsed,
            edge: Edge::None,
            hidden: false,
        };
        assert!(!state_view(&visible, &settings).hidden_to_tray);
    }

    #[test]
    fn old_pet_json_with_behaviour_fields_still_loads_placement() {
        let dir = std::env::temp_dir().join(format!("am-pet-legacy-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("pet.json");
        std::fs::write(
            &path,
            r#"{"hidden":true,"position":{"screen_key":"main","rel_x":0.9,"rel_y":0.4,"edge":"right"},
                "hover_expand_ms":999,"collapse_delay_ms":1111,"max_visible_rows":8}"#,
        )
        .unwrap();
        let store = PetStore::new(path);
        let prefs = store.load();
        assert!(prefs.hidden);
        assert_eq!(prefs.position.screen_key, "main");
        assert!((prefs.position.rel_x - 0.9).abs() < f64::EPSILON);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_handles_negative_coordinates_of_stacked_displays() {
        // Secondary display placed above the primary: y is negative.
        let screens = vec![
            screen("built-in", 0.0, 0.0, 1920.0, 1080.0),
            screen("above", 0.0, -1440.0, 1920.0, 1440.0),
        ];
        let pos = PetPosition {
            screen_key: "above".into(),
            rel_x: 1.0,
            rel_y: 0.5,
            edge: Edge::Right,
        };
        let r = restore_rect(&pos, (72.0, 72.0), &screens);
        assert!(r.y < 0.0, "expected negative y, got {:?}", r);
        assert!(r.y >= -1440.0 && r.bottom() <= 0.0);
        assert_eq!(r.right(), 1920.0);
    }

    #[test]
    fn restore_falls_back_when_saved_screen_disappeared() {
        let screens = vec![screen("only", 0.0, 0.0, 1440.0, 900.0)];
        let pos = PetPosition {
            screen_key: "unplugged".into(),
            rel_x: 1.0,
            rel_y: 0.99,
            edge: Edge::Right,
        };
        let r = restore_rect(&pos, (72.0, 72.0), &screens);
        assert!(r.x >= 0.0 && r.right() <= 1440.0);
        assert!(r.y >= 0.0 && r.bottom() <= 900.0);
    }

    #[test]
    fn position_roundtrip_keeps_relative_place() {
        let s = screen("main", 0.0, 0.0, 1920.0, 1080.0);
        let rect = Rect::new(1848.0, 400.0, 72.0, 72.0);
        let pos = position_from_rect(&rect, &s, Edge::Right);
        let back = restore_rect(&pos, (72.0, 72.0), &[s]);
        assert!((back.x - rect.x).abs() < 0.001);
        assert!((back.y - rect.y).abs() < 0.001);
    }

    #[test]
    fn screen_pick_uses_containing_display() {
        let screens = vec![
            screen("a", 0.0, 0.0, 1920.0, 1080.0),
            screen("b", 1920.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(pick_screen(&screens, 2500.0, 300.0), 1);
        assert_eq!(pick_screen(&screens, 10.0, 10.0), 0);
    }

    #[test]
    fn store_roundtrip_in_temp_dir() {
        let dir = std::env::temp_dir().join(format!("am-pet-test-{}", std::process::id()));
        let path = dir.join("pet.json");
        let store = PetStore::new(path.clone());
        let prefs = PetPrefs {
            hidden: true,
            position: PetPosition {
                screen_key: "x".into(),
                rel_x: 0.25,
                rel_y: 0.75,
                edge: Edge::Left,
            },
            ..Default::default()
        };
        store.save(&prefs);
        let loaded = store.load();
        assert!(loaded.hidden);
        assert_eq!(loaded.position.screen_key, "x");
        assert!((loaded.position.rel_x - 0.25).abs() < f64::EPSILON);
        let _ = std::fs::remove_dir_all(dir);
    }
}
