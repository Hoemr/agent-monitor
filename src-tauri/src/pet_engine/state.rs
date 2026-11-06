//! State-image pet backend — Familiar pack format (`pack.json` + one image
//! per state). This is the second proof of the [`super::backend::PetBackend`]
//! seam: a pet with no physics and no walking, just expressive states
//! (idle / working / celebrating / alarmed / …) that map directly onto the
//! monitor's moods. Two-frame APNG sprites animate themselves in the webview,
//! so the backend only decides WHICH state is showing.

use super::backend::{BackendInfo, MenuAction, PetBackend, PetFrame, WindowPolicy};
use super::behavior::Mood;
use super::pack::png_size;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    id: String,
    name: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    source: Option<String>,
    states: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct StatePack {
    /// Pack id (settings key; directory name).
    pub name: String,
    pub display_name: String,
    /// state name → image file name (`idle.png`).
    pub states: BTreeMap<String, String>,
    /// image file name → PNG bytes.
    pub images: BTreeMap<String, Vec<u8>>,
    pub frame_size: (u32, u32),
    pub author: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub warnings: Vec<String>,
}

impl StatePack {
    pub fn image_by_state(&self, state: &str) -> Option<Vec<u8>> {
        let file = self.states.get(state)?;
        self.images.get(file).cloned()
    }
}

/// Parse a state pack from in-memory files (keyed `pack.json`, `<state>.png`).
pub fn load_state_pack(name: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<StatePack, String> {
    let manifest_raw = files
        .get("pack.json")
        .ok_or_else(|| "缺少 pack.json".to_string())?;
    let manifest: Manifest = serde_json::from_slice(manifest_raw)
        .map_err(|e| format!("pack.json 解析失败: {e}"))?;
    if manifest.states.is_empty() || !manifest.states.contains_key("idle") {
        return Err("states 至少需要 idle".into());
    }
    let mut images = BTreeMap::new();
    let mut frame_size = (0u32, 0u32);
    let mut warnings = Vec::new();
    for (path, bytes) in files {
        if !path.ends_with(".png") && !path.ends_with(".PNG") {
            continue;
        }
        if frame_size.0 == 0 {
            if let Some(size) = png_size(bytes) {
                frame_size = size;
            }
        }
        images.insert(path.clone(), bytes.clone());
    }
    if images.is_empty() {
        return Err("没有任何 PNG 状态图".into());
    }
    for (state, file) in &manifest.states {
        if !images.contains_key(file) {
            warnings.push(format!("状态 {state} 引用了缺失的图片 {file}"));
        }
    }
    Ok(StatePack {
        name: name.to_string(),
        display_name: manifest.name,
        states: manifest.states,
        images,
        frame_size,
        author: manifest.author,
        license: manifest.license,
        source: manifest.source,
        warnings,
    })
}

/// Read a state pack from a directory on disk.
pub fn load_state_pack_dir(dir: &PathBuf) -> Result<StatePack, String> {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "pack".into());
    let mut files = BTreeMap::new();
    collect(dir, &mut files)?;
    load_state_pack(&name, &files)
}

fn collect(dir: &PathBuf, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let key = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            collect(&path, out)?;
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.insert(key, bytes);
        }
    }
    Ok(())
}

// ── Registry (mirrors the shimeji pack cache) ────────────────────────

fn cache() -> &'static Mutex<BTreeMap<String, StatePack>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, StatePack>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn state_pack_dir(name: &str) -> Option<PathBuf> {
    super::pack::pets_dir().map(|d| d.join(name))
}

/// Built-in state packs embedded in the binary. See docs/ASSETS.md for
/// individual artwork provenance. Large community packs load from `pets/`.
pub fn builtin_state_pack_files() -> Vec<(&'static str, BTreeMap<String, Vec<u8>>)> {
    macro_rules! state_pack {
        ($dir:literal, [$($file:literal),* $(,)?]) => {{
            let mut files = BTreeMap::new();
            files.insert("pack.json".into(), include_bytes!(concat!("../../pets/", $dir, "/pack.json")).to_vec());
            $( files.insert($file.into(), include_bytes!(concat!("../../pets/", $dir, "/", $file)).to_vec()); )*
            files
        }};
    }
    vec![
        ("nailong", state_pack!("nailong", [
            "idle.png", "working.png", "thinking.png", "interacting.png",
            "celebrating.png", "alarmed.png", "sleeping.png", "watching.png"
        ])),
        ("doraemon", state_pack!("doraemon", [
            "idle.png", "working.png", "thinking.png", "interacting.png",
            "celebrating.png", "alarmed.png", "sleeping.png", "watching.png"
        ])),
        ("tihun", state_pack!("tihun", [
            "idle.png", "working.png", "thinking.png", "interacting.png",
            "celebrating.png", "alarmed.png", "sleeping.png", "watching.png"
        ])),
        (
            "british-blue",
            state_pack!("british-blue", [
                "idle.png", "working.png", "thinking.png", "interacting.png",
                "celebrating.png", "alarmed.png", "sleeping.png", "watching.png"
            ]),
        ),
    ]
}

fn load_state_uncached(name: &str) -> Result<StatePack, String> {
    for (builtin, files) in builtin_state_pack_files() {
        if builtin == name {
            return load_state_pack(name, &files);
        }
    }
    let dir = state_pack_dir(name).ok_or_else(|| "pets 目录不可用".to_string())?;
    load_state_pack_dir(&dir)
}

pub fn load_state_pack_cached(name: &str) -> Result<StatePack, String> {
    if let Ok(cache) = cache().lock() {
        if let Some(hit) = cache.get(name) {
            return Ok(hit.clone());
        }
    }
    let pack = load_state_uncached(name)?;
    if let Ok(mut cache) = cache().lock() {
        cache.insert(name.to_string(), pack.clone());
    }
    Ok(pack)
}

pub fn invalidate_state_pack(name: &str) {
    if let Ok(mut cache) = cache().lock() {
        cache.remove(name);
    }
}

/// PNG bytes for a state name (serves `get_pet_pack_image` for this kind).
pub fn state_pack_image(name: &str, state: &str) -> Option<Vec<u8>> {
    load_state_pack_cached(name).ok()?.image_by_state(state)
}

// ── The backend ──────────────────────────────────────────────────────

/// Monitor mood → the state that expresses it. States the pack does not
/// ship fall back to `idle`.
fn mood_state(mood: Mood) -> &'static str {
    match mood {
        Mood::Idle => "idle",
        Mood::Working => "working",
        Mood::Celebrate => "celebrating",
        Mood::Distressed => "alarmed",
        Mood::Attention => "watching",
    }
}

/// Quiet activities before the pet settles down for a nap (40 ms per tick).
/// These are distinct from task moods and never pretend that a task is running.
fn idle_state(ticks: u32) -> &'static str {
    match ticks {
        250..=324 => "watching", // look around at 10 s, then settle
        750..=824 => "thinking", // daydream briefly at 30 s
        1500.. => "sleeping",   // nap after 60 s
        _ => "idle",
    }
}

#[derive(Debug, Clone)]
pub struct StateBackend {
    pack: StatePack,
    mood: Mood,
    current: String,
    /// While >0 an action/mood overlay holds the current state.
    hold_ticks: u32,
    /// Set when the mood wants a different state; consumed by `tick` so the
    /// host always learns about state changes through frames.
    dirty: bool,
    tick_count: u64,
    idle_ticks: u32,
}

/// Re-announce the current state every N ticks: the webview's listener may
/// register after the initial frame was emitted (launch race), and without a
/// heartbeat the pet would stay invisible forever.
const STATE_HEARTBEAT_TICKS: u64 = 25; // 1 s

// Only expose interactions backed by artwork in the selected pack.
const STATE_ACTIONS: [(&str, &str, &str, u32); 7] = [
    ("interact", "interacting", "menuPet", 63),
    ("celebrate", "celebrating", "menuCelebrate", 75),
    ("nap", "sleeping", "menuNap", 375),
    ("watch", "watching", "menuWatch", 150),
    ("think", "thinking", "menuThink", 125),
    ("work", "working", "menuWork", 150),
    ("surprise", "alarmed", "menuSurprise", 63),
];

impl StateBackend {
    pub fn new(pack: StatePack) -> Self {
        // `current` starts empty: the first tick MUST announce the initial
        // state (want != current), or the frontend never draws anything.
        StateBackend {
            pack,
            mood: Mood::Idle,
            current: String::new(),
            hold_ticks: 0,
            dirty: true,
            tick_count: 0,
            idle_ticks: 0,
        }
    }

    fn resolve<'a>(&self, state: &'a str) -> &'a str {
        if self.pack.states.contains_key(state) { state } else { "idle" }
    }

    fn has(&self, state: &str) -> bool {
        self.pack.states.contains_key(state)
    }

    /// Show `state` for `ticks`, then fall back to the mood state.
    fn overlay(&mut self, state: &str, ticks: u32) -> bool {
        if !self.has(state) {
            return false;
        }
        self.current = state.to_string();
        self.idle_ticks = 0;
        self.hold_ticks = ticks;
        self.dirty = true;
        true
    }

    fn frame(&self) -> PetFrame {
        PetFrame::State { state: self.current.clone() }
    }
}

impl PetBackend for StateBackend {
    fn info(&self) -> BackendInfo {
        let menu = self.menu_actions();
        let actions = menu.iter().map(|m| m.action.clone()).collect();
        BackendInfo {
            name: self.pack.name.clone(),
            display_name: Some(self.pack.display_name.clone()),
            kind: "state".into(),
            builtin: builtin_state_pack_files().iter().any(|(n, _)| *n == self.pack.name),
            author: self.pack.author.clone(),
            license: self.pack.license.clone(),
            source: self.pack.source.clone(),
            frame_size: self.pack.frame_size,
            window_size: self.window_size(),
            warnings: self.pack.warnings.clone(),
            menu,
            actions,
        }
    }

    fn window_policy(&self) -> WindowPolicy {
        // The state pet is the classic pet with different artwork: the
        // window geometry (snap, edge-hide sliver, size) is entirely the
        // host's classic pipeline; the backend only paints states.
        WindowPolicy::Fixed
    }

    fn window_size(&self) -> f64 {
        0.0 // follow the user's collapsed size, like the classic capsule
    }

    fn set_world(&mut self, _world: super::behavior::World) {}

    fn set_mood(&mut self, mood: Mood) {
        // Pending questions/errors must not stay hidden behind a manual nap.
        if matches!(mood, Mood::Attention | Mood::Distressed) && self.hold_ticks > 0 {
            self.hold_ticks = 0;
            self.dirty = true;
        }
        if mood == self.mood {
            return;
        }
        self.mood = mood;
        self.idle_ticks = 0;
        // Steady moods retarget on the next tick (unless an action overlay
        // runs); the frame — not this setter — tells the frontend.
        if self.hold_ticks == 0 {
            self.dirty = true;
        }
    }

    fn tick(&mut self) -> Option<PetFrame> {
        self.tick_count = self.tick_count.wrapping_add(1);
        let heartbeat = self.tick_count % STATE_HEARTBEAT_TICKS == 0;
        if self.hold_ticks > 0 {
            let announce = self.dirty;
            self.dirty = false;
            self.hold_ticks -= 1;
            if self.hold_ticks == 0 {
                self.dirty = true;
            }
            return if announce || heartbeat { Some(self.frame()) } else { None };
        }
        if self.mood == Mood::Idle {
            self.idle_ticks = self.idle_ticks.saturating_add(1);
        }
        let desired = if self.mood == Mood::Idle { idle_state(self.idle_ticks) } else { mood_state(self.mood) };
        let want = self.resolve(desired);
        self.dirty = false;
        if want != self.current {
            self.current = want.to_string();
            return Some(self.frame());
        }
        if heartbeat {
            return Some(self.frame());
        }
        None
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        STATE_ACTIONS.iter()
            .filter(|(_, state, _, _)| self.has(state))
            .map(|(action, _, label, _)| MenuAction {
                action: (*action).into(), label_key: (*label).into(),
            })
            .chain(std::iter::once(MenuAction {
                action: "cheer_up".into(), label_key: "menuResume".into(),
            }))
            .collect()
    }

    fn perform(&mut self, action: &str) -> bool {
        if action == "cheer_up" {
            self.hold_ticks = 0;
            self.dirty = true;
            return true;
        }
        if let Some((_, state, _, ticks)) = STATE_ACTIONS.iter().find(|(id, _, _, _)| *id == action) {
            return self.overlay(state, *ticks);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fixture() -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        files.insert(
            "pack.json".into(),
            br#"{
                "format_version": 1, "id": "t", "name": "Test Cat",
                "states": {"idle": "idle.png", "working": "working.png", "alarmed": "alarmed.png"}
            }"#
            .to_vec(),
        );
        for f in ["idle.png", "working.png", "alarmed.png"] {
            files.insert(
                f.into(),
                vec![
                    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48,
                    0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89,
                ],
            );
        }
        files
    }

    #[test]
    fn parses_manifest_and_flags_missing_states() {
        let pack = load_state_pack("t", &fixture()).unwrap();
        assert_eq!(pack.display_name, "Test Cat");
        assert_eq!(pack.frame_size, (1, 1));
        assert!(pack.states.contains_key("idle"));
        let mut broken = fixture();
        broken.remove("alarmed.png");
        let pack = load_state_pack("t", &broken).unwrap();
        assert!(pack.warnings.iter().any(|w| w.contains("alarmed")));
    }

    #[test]
    fn manifest_without_idle_is_rejected() {
        let mut files = fixture();
        files.insert(
            "pack.json".into(),
            br#"{"format_version":1,"id":"t","name":"t","states":{"working":"working.png"}}"#.to_vec(),
        );
        assert!(load_state_pack("t", &files).is_err());
    }

    #[test]
    fn mood_maps_to_states_with_idle_fallback() {
        let backend = StateBackend::new(load_state_pack("t", &fixture()).unwrap());
        let info = backend.info();
        assert_eq!(info.kind, "state");
        // watching is not shipped → resolves to idle.
        assert_eq!(backend.resolve("watching"), "idle");
        assert_eq!(backend.resolve("working"), "working");
    }

    #[test]
    fn first_tick_announces_the_initial_state() {
        // Regression: activation starts at "idle" and the mood is Idle, so
        // nothing "changes" — without the initial frame the pet is invisible.
        let mut backend = StateBackend::new(load_state_pack("t", &fixture()).unwrap());
        match backend.tick() {
            Some(PetFrame::State { state }) => assert_eq!(state, "idle"),
            other => panic!("expected the initial state frame, got {other:?}"),
        }
        assert!(backend.tick().is_none());
    }

    #[test]
    fn tick_emits_frame_only_on_state_change() {
        let mut backend = StateBackend::new(load_state_pack("t", &fixture()).unwrap());
        assert!(backend.tick().is_some()); // initial announcement (count 1)
        // Quiet until the heartbeat re-announces (1 s cadence at count 25).
        for _ in 0..(STATE_HEARTBEAT_TICKS - 2) {
            assert!(backend.tick().is_none());
        }
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "idle"));
        backend.set_mood(Mood::Working);
        match backend.tick() {
            Some(PetFrame::State { state }) => assert_eq!(state, "working"),
            other => panic!("expected state frame, got {other:?}"),
        }
        assert!(backend.tick().is_none()); // unchanged
        backend.set_mood(Mood::Idle);
        match backend.tick() {
            Some(PetFrame::State { state }) => assert_eq!(state, "idle"),
            other => panic!("expected state frame, got {other:?}"),
        }
    }

    #[test]
    fn action_overlays_hold_then_return_to_mood() {
        let mut backend = StateBackend::new(load_state_pack("t", &fixture()).unwrap());
        assert!(backend.tick().is_some()); // consume the initial announcement
        assert!(!backend.perform("celebrate")); // celebrating not shipped
        // interacting not shipped either → overlay resolves to idle → no change.
        assert!(backend.tick().is_none());
        backend.set_mood(Mood::Working);
        backend.tick();
        // alarmed IS shipped: distressed overlay holds, then decays back.
        backend.set_mood(Mood::Distressed);
        backend.tick();
        assert!(matches!(backend.tick(), Some(PetFrame::State { .. })) || backend.current == "alarmed");
    }

    #[test]
    fn interactions_emit_immediately_repeat_and_resume_live_mood() {
        let files = builtin_state_pack_files();
        let (_, files) = files.iter().find(|(name, _)| *name == "british-blue").unwrap();
        let mut backend = StateBackend::new(load_state_pack("british-blue", files).unwrap());
        backend.set_mood(Mood::Working);
        backend.tick();
        for (action, state, _, _) in STATE_ACTIONS {
            assert!(backend.menu_actions().iter().any(|m| m.action == action));
            assert!(backend.perform(action));
            assert!(matches!(backend.tick(), Some(PetFrame::State { state: s }) if s == state));
            assert!(backend.perform(action));
            assert!(matches!(backend.tick(), Some(PetFrame::State { state: s }) if s == state));
        }
        assert!(backend.perform("cheer_up"));
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "working"));
    }

    #[test]
    fn interaction_expires_and_unsupported_actions_leave_state_unchanged() {
        let mut backend = StateBackend::new(load_state_pack("t", &fixture()).unwrap());
        backend.set_mood(Mood::Working);
        backend.tick();
        assert!(!backend.perform("nap"));
        assert!(!backend.perform("unknown"));
        assert_eq!(backend.current, "working");
        assert!(backend.perform("surprise"));
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "alarmed"));
        for _ in 1..63 { backend.tick(); }
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "working"));
    }

    #[test]
    fn idle_activities_stay_quiet_and_new_work_or_questions_interrupt_them() {
        let files = builtin_state_pack_files();
        let (_, files) = files.iter().find(|(name, _)| *name == "british-blue").unwrap();
        let mut backend = StateBackend::new(load_state_pack("british-blue", files).unwrap());
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1500 {
            if let Some(PetFrame::State { state }) = backend.tick() { seen.insert(state); }
        }
        assert!(seen.contains("idle") && seen.contains("watching") && seen.contains("thinking") && seen.contains("sleeping"));
        assert!(!seen.contains("working") && !seen.contains("alarmed"));
        backend.set_mood(Mood::Working);
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "working"));
        backend.set_mood(Mood::Idle);
        for _ in 0..250 { backend.tick(); }
        assert_eq!(backend.current, "watching");
        backend.set_mood(Mood::Attention);
        backend.tick();
        assert_eq!(backend.idle_ticks, 0);
        assert_eq!(backend.mood, Mood::Attention);
        for _ in 0..1500 { backend.tick(); }
        assert_eq!(backend.current, "watching"); // an actual question never turns into a nap
    }

    #[test]
    fn idle_pet_sleeps_and_task_activity_wakes_it() {
        let files = builtin_state_pack_files();
        let (_, files) = files.iter().find(|(name, _)| *name == "british-blue").unwrap();
        let mut backend = StateBackend::new(load_state_pack("british-blue", files).unwrap());
        for _ in 0..1500 { backend.tick(); }
        assert_eq!(backend.current, "sleeping");
        backend.set_mood(Mood::Working);
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "working"));
        backend.set_mood(Mood::Idle);
        assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == "idle"));
    }

    #[test]
    fn questions_and_errors_interrupt_manual_actions() {
        let files = builtin_state_pack_files();
        let (_, files) = files.iter().find(|(name, _)| *name == "british-blue").unwrap();
        let mut backend = StateBackend::new(load_state_pack("british-blue", files).unwrap());
        for (mood, expected) in [(Mood::Attention, "watching"), (Mood::Distressed, "alarmed")] {
            assert!(backend.perform("nap"));
            backend.tick();
            backend.set_mood(mood);
            assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == expected));
            // Repeated manual actions cannot mask a still-pending question/error.
            assert!(backend.perform("nap"));
            backend.tick();
            backend.set_mood(mood);
            assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == expected));
        }
    }

    #[test]
    fn builtin_packs_parse() {
        for (name, files) in builtin_state_pack_files() {
            let pack = load_state_pack(name, &files).expect(name);
            assert!(pack.states.contains_key("idle"), "{name}");
            assert!(pack.frame_size.0 >= 256, "{name} art should be high-res");
            assert!(pack.license.is_some(), "{name} must carry its license");
        }
    }
}
