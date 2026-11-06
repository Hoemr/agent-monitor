//! The pet backend contract — the single seam between the host (window
//! plumbing, moods, menus) and every pet implementation.
//!
//! Adding a new pet kind means implementing [`PetBackend`] and registering a
//! loader: the host never branches on the concrete kind. Two backends ship
//! today: the Shimeji physics pet ([`super::behavior`]) and the state-image
//! pet ([`super::state`], Familiar-pack format). Live2D/GIF/whatever comes
//! next plugs in the same way.

use super::behavior::{Frame, Mood, World};

/// Which window geometry model the backend needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPolicy {
    /// The window follows the backend's anchor every tick (roaming pet).
    FollowsAnchor,
    /// The window stays where the user placed it; the pet fills it.
    Fixed,
}

/// What the frontend should draw this tick. The payload is backend-specific;
/// the frontend dispatches on `kind`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PetFrame {
    /// One animation frame of a roaming sprite (image key + anchors).
    Sprite {
        image: String,
        anchor: (f64, f64),
        image_anchor: (f64, f64),
        facing_right: bool,
    },
    /// A named state image (Familiar-style pack); the frontend stretches it
    /// to the window. APNG frames animate by themselves.
    State { state: String },
}

/// A right-click menu entry: engine action id + i18n key for the label.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MenuAction {
    pub action: String,
    /// i18n key resolved by the frontend (`menu*` keys in main.js).
    pub label_key: String,
}

/// Everything the host asks of a pet implementation, in the order the host
/// calls it: `set_world`/`set_mood` as context changes, `tick` every frame,
/// menu/drag calls on user interaction.
pub trait PetBackend: Send {
    /// Static description for the settings UI and pack listing.
    fn info(&self) -> BackendInfo;
    fn window_policy(&self) -> WindowPolicy;
    /// Logical window size (the pet square) this backend wants.
    fn window_size(&self) -> f64;
    fn set_world(&mut self, world: World);
    fn set_mood(&mut self, mood: Mood);
    /// Advance one engine tick (40 ms). `None` = keep showing the last frame.
    fn tick(&mut self) -> Option<PetFrame>;
    /// Right-click menu, most interesting first.
    fn menu_actions(&self) -> Vec<MenuAction>;
    /// Trigger a named action; false when the backend does not know it.
    fn perform(&mut self, action: &str) -> bool;
    /// Drag lifecycle (physics backends); state pets may ignore these.
    fn grab(&mut self, _point: (f64, f64)) {}
    fn drag_to(&mut self, _point: (f64, f64)) {}
    /// The user let go at `anchor` with `velocity` (px per tick, from the
    /// window-drag trajectory).
    fn drag_ended(&mut self, _anchor: (f64, f64), _velocity: (f64, f64)) {}
    /// Teleport to the middle of the floor (roaming pets only).
    fn center(&mut self) {}
}

/// Static backend description surfaced in Settings and pack listings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendInfo {
    /// Pack name (matches the directory / settings key).
    pub name: String,
    pub display_name: Option<String>,
    /// `shimeji` | `state` — lets the UI tag pack rows without knowing kinds.
    pub kind: String,
    pub builtin: bool,
    /// Author / license / source from the pack metadata, shown in Settings.
    pub author: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub frame_size: (u32, u32),
    /// Logical window edge the backend wants (state pets: own size).
    pub window_size: f64,
    pub warnings: Vec<String>,
    /// The right-click menu, backend-defined: id + i18n key. The frontend
    /// never hardcodes pet kinds' menus.
    pub menu: Vec<MenuAction>,
    /// Convenience: the menu's action ids (settings picker compatibility).
    pub actions: Vec<String>,
}

impl PetFrame {
    /// Convert a roaming sprite frame into the generic payload.
    pub fn from_sprite(frame: Frame) -> Self {
        PetFrame::Sprite {
            image: frame.image,
            anchor: frame.anchor,
            image_anchor: frame.image_anchor,
            facing_right: frame.facing_right,
        }
    }
}
