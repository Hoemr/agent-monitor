//! Dynamic-Island mode (macOS): notch geometry + native panel styling.
//!
//! The island window itself is a plain borderless Tauri window; this module
//! only supplies what Tauri cannot: the hardware-notch geometry (via
//! `NSScreen`) and the window-level / collection-behaviour promotions that
//! keep the panel glued to the menu bar on every Space and above full-screen
//! apps. Rect math and mode plumbing live in `lib.rs`; island UI in
//! `src/island.*` (frontend).

use serde::Serialize;

/// Geometry of the display that hosts the island, in logical points. Cocoa's
/// bottom-left global origin is converted to the top-left origin Tauri/tao
/// uses, so a pill rect can be derived without further coordinate translation.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct IslandMetrics {
    /// Hardware notch width on notched displays; virtual-notch width on the
    /// floating fallback.
    pub notch_w: f64,
    /// Actual menu-bar / notch row height, in points.
    pub bar_h: f64,
    /// Backing scale factor of the hosting screen.
    pub scale: f64,
    /// Top-left corner of the hosting screen, global logical points.
    pub origin_x: f64,
    pub origin_y: f64,
    /// Horizontal centre of the notch, global logical points.
    pub center_x: f64,
    /// Available display height and shared frontend panel width, in points.
    pub screen_h: f64,
    pub panel_w: f64,
}

/// Displays without a notch have no empty hardware region in the pill.
const VIRTUAL_NOTCH_W: f64 = 0.0;

/// Locate the display that hosts the island: the notched screen when one is
/// attached, otherwise the primary display with a floating virtual notch.
/// Always returns `Some` on a working macOS session — island mode never needs
/// to bail out for lacking a notch.
#[cfg(target_os = "macos")]
pub fn island_display_geometry() -> Option<IslandMetrics> {
    use objc2_app_kit::{NSScreen, NSStatusBar};
    use objc2_foundation::MainThreadMarker;

    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let count = screens.len();
    for i in 0..count {
        let screen = screens.objectAtIndex(i);
        if screen.safeAreaInsets().top > 0.0 {
            let frame = screen.frame();
            let left = screen.auxiliaryTopLeftArea();
            let right = screen.auxiliaryTopRightArea();
            let primary = screens.firstObject()?;
            let primary_h = primary.frame().size.height;
            return Some(IslandMetrics {
                notch_w: frame.size.width - left.size.width - right.size.width,
                bar_h: screen.safeAreaInsets().top,
                scale: screen.backingScaleFactor(),
                origin_x: frame.origin.x,
                origin_y: primary_h - (frame.origin.y + frame.size.height),
                center_x: frame.origin.x + frame.size.width / 2.0,
                screen_h: frame.size.height,
                panel_w: super::ISLAND_PANEL_W_PT,
            });
        }
    }
    let primary = screens.firstObject()?;
    let frame = primary.frame();
    let visible = primary.visibleFrame();
    // The reserved top inset includes the menu bar's display-specific padding.
    // With automatic hiding enabled, retain the system status item's height.
    let bar_h = ((frame.origin.y + frame.size.height)
        - (visible.origin.y + visible.size.height))
        .max(NSStatusBar::systemStatusBar().thickness());
    Some(IslandMetrics {
        notch_w: VIRTUAL_NOTCH_W,
        bar_h,
        scale: primary.backingScaleFactor(),
        origin_x: frame.origin.x,
        origin_y: primary.frame().size.height - (frame.origin.y + frame.size.height),
        center_x: frame.origin.x + frame.size.width / 2.0,
        screen_h: frame.size.height,
        panel_w: super::ISLAND_PANEL_W_PT,
    })
}

/// Off macOS the island simply does not exist.
#[cfg(not(target_os = "macos"))]
pub fn island_display_geometry() -> Option<IslandMetrics> {
    None
}

/// Promote the island window to a menu-bar-level, all-Spaces, full-screen-
/// friendly panel. Idempotent; call after creation and whenever re-shown.
#[cfg(target_os = "macos")]
pub fn apply_panel_style(win: &tauri::WebviewWindow) -> Result<(), String> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let raw = win.ns_window().map_err(|e| format!("ns_window: {e}"))?;
    // SAFETY: the pointer is a valid NSWindow borrowed from tao. `from_raw`
    // treats it as +1; `forget` never releases, so tao keeps sole ownership.
    let Some(ns) = (unsafe { Retained::from_raw(raw.cast::<NSWindow>()) }) else {
        return Err("ns_window: null".into());
    };
    // `NSStatusBarWindowLevel`: the pill must draw above the menu bar itself.
    ns.setLevel(25);
    // Borderless windows default to NO here; without it the webview never
    // sees mouse-moved events and the hover-expand never fires.
    ns.setAcceptsMouseMovedEvents(true);
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    std::mem::forget(ns);
    Ok(())
}
