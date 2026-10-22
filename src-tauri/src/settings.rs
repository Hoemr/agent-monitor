//! User settings: the single source of truth for configurable behaviour.
//!
//! Persisted to `~/Library/Application Support/agent-monitor/settings.json`
//! (override the directory with `AGENT_MONITOR_DATA_DIR` for tests/acceptance):
//!   * missing fields fall back to defaults (forward/backward compatible),
//!   * unknown fields are ignored,
//!   * every numeric value is clamped/snapped on load and on save,
//!   * writes are atomic (tmp + rename),
//!   * a corrupt file is moved to `settings.json.bak` and defaults are used so a
//!     bad edit can never prevent the app from starting.
//!
//! Placement (`pet.json`) deliberately stays in its own file: it changes on every
//! drag and is high-frequency state, not user configuration. Both files live in
//! the same data directory and are loaded independently.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SETTINGS_VERSION: u32 = 1;
pub const MAX_VISIBLE_ROWS_CAP: u32 = 8;
pub const RETENTION_CHOICES: [u32; 4] = [1, 6, 24, 72];
pub const ROW_CHOICES: [u32; 3] = [3, 5, 8];
pub const HOVER_RANGE: (u64, u64) = (100, 1000);
pub const COLLAPSE_RANGE: (u64, u64) = (300, 2000);
pub const SNAP_RANGE: (u32, u32) = (0, 40);
pub const SLIVER_RANGE: (u32, u32) = (8, 24);
/// Collapsed/snapped pet edge length in logical points.
pub const COLLAPSED_SIZE_RANGE: (u32, u32) = (48, 96);
/// Idle time after snapping before the pet hides itself; 0 = never.
pub const AUTO_HIDE_RANGE: (u32, u32) = (0, 60);
/// Minimum allowed active refresh interval (seconds). Below this the scanner
/// would burn CPU for no user-visible gain.
pub const MIN_REFRESH_SECS: u64 = 2;

// ── Enums (serialised as stable snake_case strings) ─────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    System,
    En,
    Zh,
}

impl Default for Language {
    fn default() -> Self {
        Language::System
    }
}

impl Language {
    /// Resolve `system` against the OS language. Only the two shipped locales
    /// exist; everything else falls back to English.
    pub fn resolve(self, system_is_zh: bool) -> Language {
        match self {
            Language::System => {
                if system_is_zh {
                    Language::Zh
                } else {
                    Language::En
                }
            }
            other => other,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::En => "en",
            Language::Zh => "zh",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    System,
    Light,
    Dark,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance::System
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshInterval {
    Auto,
    #[serde(rename = "3s")]
    Sec3,
    #[serde(rename = "5s")]
    Sec5,
    #[serde(rename = "10s")]
    Sec10,
}

impl Default for RefreshInterval {
    fn default() -> Self {
        RefreshInterval::Auto
    }
}

impl RefreshInterval {
    /// `None` = auto (visible 5 s / hidden or empty 30 s).
    pub fn seconds(self) -> Option<u64> {
        match self {
            RefreshInterval::Auto => None,
            RefreshInterval::Sec3 => Some(3),
            RefreshInterval::Sec5 => Some(5),
            RefreshInterval::Sec10 => Some(10),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            RefreshInterval::Auto => "auto",
            RefreshInterval::Sec3 => "3s",
            RefreshInterval::Sec5 => "5s",
            RefreshInterval::Sec10 => "10s",
        }
    }
}

/// Which artwork the collapsed pet / edge-hidden tab uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconStyle {
    /// Built-in three-bar glyph.
    Builtin,
    /// User-supplied images from the icon slots (fallback: built-in glyph).
    Custom,
}

impl Default for IconStyle {
    fn default() -> Self {
        IconStyle::Builtin
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Animations {
    System,
    Always,
    Off,
}

impl Default for Animations {
    fn default() -> Self {
        Animations::System
    }
}

/// Per-agent collection switches. Keys match the adapter keys used in
/// `TaskRecord::agent_key`; `claude_code` maps to the `claude-code` agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnabledAgents {
    pub codex: bool,
    pub dsh: bool,
    pub zcode: bool,
    pub claude_code: bool,
}

impl Default for EnabledAgents {
    fn default() -> Self {
        Self {
            codex: true,
            dsh: true,
            zcode: true,
            claude_code: true,
        }
    }
}

impl EnabledAgents {
    pub fn enabled(&self, agent_key: &str) -> bool {
        match agent_key {
            "codex" => self.codex,
            "dsh" => self.dsh,
            "zcode" => self.zcode,
            "claude-code" | "claude_code" => self.claude_code,
            // Unknown adapters (simulated/future) are never disabled by a
            // settings file that predates them.
            _ => true,
        }
    }

    pub fn any(&self) -> bool {
        self.codex || self.dsh || self.zcode || self.claude_code
    }
}

// ── Settings ────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,

    // General / appearance
    pub language: Language,
    pub appearance: Appearance,
    pub show_summary: bool,
    pub show_jump_level: bool,
    /// Show the "new completions since you last looked" indicator while the pet
    /// is collapsed/hidden.
    pub new_badge: bool,

    // Attention / pending questions (see docs/ATTENTION.md)
    /// Show a macOS notification when a session newly blocks on a question.
    pub notify_on_question: bool,
    /// Play the system "Glass" sound with the alert (also when notifications
    /// are disabled, via `afplay`).
    pub alert_sound: bool,
    /// Reveal/expand the pet when a new question appears (never focuses it).
    pub auto_show_on_question: bool,

    // Monitoring
    pub refresh_interval: RefreshInterval,
    pub retention_hours: u32,
    pub enabled_agents: EnabledAgents,
    pub max_visible_rows: u32,

    // Desktop pet
    pub hover_expand_ms: u64,
    pub collapse_delay_ms: u64,
    pub snap_distance: u32,
    pub hidden_sliver: u32,
    /// Edge length of the collapsed/snapped pet (logical points).
    pub collapsed_size: u32,
    /// After snapping, hide to the edge/tray after this many idle seconds.
    /// `0` disables the behaviour (default).
    pub auto_hide_after_s: u32,
    pub edge_hide_enabled: bool,
    pub animations: Animations,
    /// Built-in glyph vs. uploaded custom images.
    pub icon_style: IconStyle,

    // Jump
    pub toast_on_degraded: bool,
    pub auto_dismiss_invalid: bool,

    // Updates
    /// Check GitHub Releases once ~4 s after launch (silent when up to date or
    /// offline). Manual checks in Settings → Updates are always allowed.
    pub auto_check_updates: bool,

    // Migration bookkeeping
    pub legacy_imported: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            language: Language::System,
            appearance: Appearance::System,
            show_summary: true,
            show_jump_level: true,
            new_badge: true,
            notify_on_question: true,
            alert_sound: true,
            auto_show_on_question: true,
            refresh_interval: RefreshInterval::Auto,
            retention_hours: 6,
            enabled_agents: EnabledAgents::default(),
            max_visible_rows: 5,
            hover_expand_ms: 150,
            collapse_delay_ms: 800,
            snap_distance: 16,
            hidden_sliver: 14,
            collapsed_size: 72,
            auto_hide_after_s: 0,
            edge_hide_enabled: true,
            animations: Animations::System,
            icon_style: IconStyle::Builtin,
            toast_on_degraded: true,
            auto_dismiss_invalid: false,
            auto_check_updates: true,
            legacy_imported: false,
        }
    }
}

impl Settings {
    /// Clamp every value into its supported range. Called on load, after every
    /// `set_settings` and before persistence, so the file and the runtime can
    /// never disagree.
    pub fn normalize(&mut self) {
        self.version = SETTINGS_VERSION;
        self.retention_hours = snap_to(self.retention_hours, &RETENTION_CHOICES);
        self.max_visible_rows = snap_to(self.max_visible_rows, &ROW_CHOICES)
            .clamp(1, MAX_VISIBLE_ROWS_CAP);
        self.hover_expand_ms = self.hover_expand_ms.clamp(HOVER_RANGE.0, HOVER_RANGE.1);
        self.collapse_delay_ms = self.collapse_delay_ms.clamp(COLLAPSE_RANGE.0, COLLAPSE_RANGE.1);
        self.snap_distance = self.snap_distance.clamp(SNAP_RANGE.0, SNAP_RANGE.1);
        self.hidden_sliver = self.hidden_sliver.clamp(SLIVER_RANGE.0, SLIVER_RANGE.1);
        self.collapsed_size = self
            .collapsed_size
            .clamp(COLLAPSED_SIZE_RANGE.0, COLLAPSED_SIZE_RANGE.1);
        self.auto_hide_after_s = self
            .auto_hide_after_s
            .clamp(AUTO_HIDE_RANGE.0, AUTO_HIDE_RANGE.1);
        // `toast_on_degraded` intentionally stays free: when off, the UI still
        // shows *failures* — only the softer "degraded" notice is suppressed.
    }

    pub fn retention_secs(&self) -> u64 {
        u64::from(self.retention_hours) * 3600
    }

    /// Configured active refresh, clamped to the hard minimum. `None` = auto.
    pub fn active_refresh_secs(&self) -> Option<u64> {
        self.refresh_interval
            .seconds()
            .map(|s| s.max(MIN_REFRESH_SECS))
    }

    /// How many rows the expanded panel should be tall for `task_count` tasks:
    /// never taller than the configured cap, never taller than the data, and at
    /// least one row.
    pub fn visible_rows(&self, task_count: usize) -> usize {
        let cap = self.max_visible_rows.clamp(1, MAX_VISIBLE_ROWS_CAP) as usize;
        task_count.max(1).min(cap)
    }

    pub fn reset_group(&mut self, group: SettingsGroup) {
        let d = Settings::default();
        match group {
            SettingsGroup::All => *self = d,
            SettingsGroup::General => {
                self.language = d.language;
                self.appearance = d.appearance;
                self.show_summary = d.show_summary;
                self.show_jump_level = d.show_jump_level;
                self.new_badge = d.new_badge;
            }
            SettingsGroup::Attention => {
                self.notify_on_question = d.notify_on_question;
                self.alert_sound = d.alert_sound;
                self.auto_show_on_question = d.auto_show_on_question;
            }
            SettingsGroup::Monitor => {
                self.refresh_interval = d.refresh_interval;
                self.retention_hours = d.retention_hours;
                self.enabled_agents = d.enabled_agents;
                self.max_visible_rows = d.max_visible_rows;
            }
            SettingsGroup::Pet => {
                self.hover_expand_ms = d.hover_expand_ms;
                self.collapse_delay_ms = d.collapse_delay_ms;
                self.snap_distance = d.snap_distance;
                self.hidden_sliver = d.hidden_sliver;
                self.collapsed_size = d.collapsed_size;
                self.auto_hide_after_s = d.auto_hide_after_s;
                self.edge_hide_enabled = d.edge_hide_enabled;
                self.animations = d.animations;
                self.icon_style = d.icon_style;
            }
            SettingsGroup::Jump => {
                self.toast_on_degraded = d.toast_on_degraded;
                self.auto_dismiss_invalid = d.auto_dismiss_invalid;
            }
            SettingsGroup::Update => {
                self.auto_check_updates = d.auto_check_updates;
            }
        }
        self.normalize();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsGroup {
    All,
    General,
    Attention,
    Monitor,
    Pet,
    Jump,
    Update,
}

impl SettingsGroup {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all" => Some(SettingsGroup::All),
            "general" => Some(SettingsGroup::General),
            "attention" => Some(SettingsGroup::Attention),
            "monitor" => Some(SettingsGroup::Monitor),
            "pet" => Some(SettingsGroup::Pet),
            "jump" => Some(SettingsGroup::Jump),
            "update" => Some(SettingsGroup::Update),
            _ => None,
        }
    }
}

fn snap_to(value: u32, choices: &[u32]) -> u32 {
    choices
        .iter()
        .copied()
        .min_by_key(|c| (*c as i64 - value as i64).abs())
        .unwrap_or(value)
}

// ── Paths + persistence ─────────────────────────────────────────

/// Data directory shared by settings, placement and dismissed-task state.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENT_MONITOR_DATA_DIR") {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Library/Application Support/agent-monitor")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn pet_path() -> PathBuf {
    data_dir().join("pet.json")
}

pub fn dismissed_path() -> PathBuf {
    data_dir().join("dismissed.json")
}

/// Result of loading the settings file. Informational only — the app always
/// continues with usable settings.
#[derive(Clone, Debug)]
pub struct LoadOutcome {
    pub settings: Settings,
    pub existed: bool,
    /// The file was unreadable/invalid and was moved aside.
    pub recovered_from_corrupt: bool,
    pub backup: Option<PathBuf>,
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_store() -> Self {
        Self::new(settings_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> LoadOutcome {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(_) => {
                return LoadOutcome {
                    settings: Settings::default(),
                    existed: false,
                    recovered_from_corrupt: false,
                    backup: None,
                }
            }
        };
        match serde_json::from_str::<Settings>(&raw) {
            Ok(mut settings) => {
                settings.normalize();
                LoadOutcome {
                    settings,
                    existed: true,
                    recovered_from_corrupt: false,
                    backup: None,
                }
            }
            Err(err) => {
                // Keep the broken file for the user; never block startup.
                let backup = self.path.with_extension("json.bak");
                let mut backed_up = false;
                if std::fs::rename(&self.path, &backup).is_ok() {
                    backed_up = true;
                    eprintln!(
                        "settings: corrupt file backed up to {} ({err})",
                        backup.display()
                    );
                }
                LoadOutcome {
                    settings: Settings::default(),
                    existed: true,
                    recovered_from_corrupt: true,
                    backup: backed_up.then_some(backup),
                }
            }
        }
    }

    pub fn load_settings(&self) -> Settings {
        self.load().settings
    }

    /// Atomic, best-effort save. A failure is reported to the caller but never
    /// panics and never blocks the UI.
    pub fn save(&self, settings: &Settings) -> bool {
        if let Some(parent) = self.path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        let Ok(json) = serde_json::to_string_pretty(settings) else {
            return false;
        };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_err() {
            return false;
        }
        std::fs::rename(&tmp, &self.path).is_ok()
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> (SettingsStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "am-settings-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        (SettingsStore::new(path), dir)
    }

    #[test]
    fn defaults_match_the_documented_contract() {
        let s = Settings::default();
        assert_eq!(s.version, 1);
        assert_eq!(s.language, Language::System);
        assert_eq!(s.appearance, Appearance::System);
        assert!(s.show_summary && s.show_jump_level);
        assert!(s.new_badge);
        assert!(s.notify_on_question);
        assert!(s.alert_sound);
        assert!(s.auto_show_on_question);
        assert_eq!(s.refresh_interval, RefreshInterval::Auto);
        assert_eq!(s.retention_hours, 6);
        assert_eq!(s.retention_secs(), 6 * 3600);
        assert!(s.enabled_agents.any());
        assert_eq!(s.max_visible_rows, 5);
        assert_eq!(s.hover_expand_ms, 150);
        assert_eq!(s.collapse_delay_ms, 800);
        assert_eq!(s.snap_distance, 16);
        assert_eq!(s.hidden_sliver, 14);
        assert_eq!(s.collapsed_size, 72);
        assert_eq!(s.auto_hide_after_s, 0);
        assert!(s.edge_hide_enabled);
        assert_eq!(s.animations, Animations::System);
        assert_eq!(s.icon_style, IconStyle::Builtin);
        assert!(s.toast_on_degraded);
        assert!(!s.auto_dismiss_invalid);
        assert!(s.auto_check_updates);
        assert!(!s.legacy_imported);
    }

    #[test]
    fn normalize_clamps_and_snaps_every_numeric_field() {
        let mut s = Settings {
            retention_hours: 999,
            max_visible_rows: 7,
            hover_expand_ms: 1,
            collapse_delay_ms: 99_999,
            snap_distance: 99,
            hidden_sliver: 1,
            collapsed_size: 999,
            auto_hide_after_s: 999,
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.retention_hours, 72);
        assert_eq!(s.max_visible_rows, 8);
        assert_eq!(s.hover_expand_ms, 100);
        assert_eq!(s.collapse_delay_ms, 2000);
        assert_eq!(s.snap_distance, 40);
        assert_eq!(s.hidden_sliver, 8);
        assert_eq!(s.collapsed_size, 96);
        assert_eq!(s.auto_hide_after_s, 60);

        let mut low = Settings {
            retention_hours: 0,
            max_visible_rows: 0,
            hover_expand_ms: 0,
            collapse_delay_ms: 0,
            snap_distance: 0,
            hidden_sliver: 0,
            collapsed_size: 0,
            auto_hide_after_s: 0,
            ..Default::default()
        };
        low.normalize();
        assert_eq!(low.retention_hours, 1);
        assert_eq!(low.max_visible_rows, 3);
        assert_eq!(low.hover_expand_ms, 100);
        assert_eq!(low.collapse_delay_ms, 300);
        assert_eq!(low.snap_distance, 0);
        assert_eq!(low.hidden_sliver, 8);
        assert_eq!(low.collapsed_size, 48);
        assert_eq!(low.auto_hide_after_s, 0);
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_fields_default() {
        let json = r#"{"language":"zh","future_field":{"nested":1},"retention_hours":24}"#;
        let s: Settings = serde_json::from_str(json).expect("partial settings must parse");
        assert_eq!(s.language, Language::Zh);
        assert_eq!(s.retention_hours, 24);
        assert_eq!(s.max_visible_rows, 5); // default
        assert_eq!(s.version, 1); // default version
    }

    #[test]
    fn refresh_intervals_serialise_as_ui_values() {
        for (ri, key, secs) in [
            (RefreshInterval::Auto, "auto", None),
            (RefreshInterval::Sec3, "3s", Some(3)),
            (RefreshInterval::Sec5, "5s", Some(5)),
            (RefreshInterval::Sec10, "10s", Some(10)),
        ] {
            assert_eq!(ri.key(), key);
            assert_eq!(ri.seconds(), secs);
            let json = serde_json::to_string(&ri).unwrap();
            assert_eq!(json, format!("\"{key}\""));
        }
    }

    #[test]
    fn language_resolution_honours_system_and_explicit_choice() {
        assert_eq!(Language::System.resolve(true), Language::Zh);
        assert_eq!(Language::System.resolve(false), Language::En);
        assert_eq!(Language::En.resolve(true), Language::En);
        assert_eq!(Language::Zh.resolve(false), Language::Zh);
    }

    #[test]
    fn visible_rows_never_exceeds_cap_or_data() {
        let s = Settings {
            max_visible_rows: 8,
            ..Default::default()
        };
        assert_eq!(s.visible_rows(20), 8);
        assert_eq!(s.visible_rows(3), 3);
        assert_eq!(s.visible_rows(0), 1);
        let five = Settings::default();
        assert_eq!(five.visible_rows(20), 5);
        assert_eq!(five.visible_rows(2), 2);
    }

    #[test]
    fn active_refresh_can_never_go_below_the_floor() {
        let mut s = Settings {
            refresh_interval: RefreshInterval::Sec3,
            ..Default::default()
        };
        assert_eq!(s.active_refresh_secs(), Some(3));
        s.refresh_interval = RefreshInterval::Auto;
        assert_eq!(s.active_refresh_secs(), None);
        assert!(MIN_REFRESH_SECS >= 2);
    }

    #[test]
    fn enabled_agents_map_keys_and_keep_unknown_agents_enabled() {
        let mut s = EnabledAgents::default();
        assert!(s.enabled("codex") && s.enabled("claude-code"));
        s.claude_code = false;
        assert!(!s.enabled("claude-code"));
        assert!(!s.enabled("claude_code"));
        assert!(s.enabled("simulated"));
        s.codex = false;
        s.dsh = false;
        s.zcode = false;
        assert!(!s.any());
    }

    #[test]
    fn group_reset_only_touches_its_own_fields() {
        let mut s = Settings {
            language: Language::Zh,
            retention_hours: 72,
            max_visible_rows: 8,
            hover_expand_ms: 500,
            toast_on_degraded: false,
            ..Default::default()
        };
        s.reset_group(SettingsGroup::Pet);
        assert_eq!(s.hover_expand_ms, 150);
        assert_eq!(s.collapsed_size, 72);
        assert_eq!(s.auto_hide_after_s, 0);
        assert_eq!(s.language, Language::Zh);
        assert_eq!(s.retention_hours, 72);
        assert!(!s.toast_on_degraded);
        s.reset_group(SettingsGroup::All);
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn store_roundtrip_is_atomic_and_reloadable() {
        let (store, dir) = temp_store("roundtrip");
        let mut s = Settings {
            language: Language::Zh,
            max_visible_rows: 8,
            enabled_agents: EnabledAgents {
                zcode: false,
                ..Default::default()
            },
            ..Default::default()
        };
        s.normalize();
        assert!(store.save(&s));
        assert!(store.path().exists());
        assert!(!store.path().with_extension("json.tmp").exists());
        assert_eq!(store.load_settings(), s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_and_keeps_a_backup() {
        let (store, dir) = temp_store("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(store.path(), "{ this is not json").unwrap();
        let out = store.load();
        assert!(out.existed);
        assert!(out.recovered_from_corrupt);
        assert_eq!(out.settings, Settings::default());
        let backup = out.backup.expect("backup file must be preserved");
        assert!(backup.exists());
        assert!(std::fs::read_to_string(&backup).unwrap().contains("not json"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_uses_defaults_without_creating_one() {
        let (store, dir) = temp_store("missing");
        let out = store.load();
        assert!(!out.existed);
        assert!(!out.recovered_from_corrupt);
        assert_eq!(out.settings, Settings::default());
        assert!(!store.path().exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn serialised_shape_is_stable_for_the_frontend() {
        let json = serde_json::to_value(Settings::default()).unwrap();
        for key in [
            "version",
            "language",
            "appearance",
            "show_summary",
            "show_jump_level",
            "new_badge",
            "notify_on_question",
            "alert_sound",
            "auto_show_on_question",
            "refresh_interval",
            "retention_hours",
            "enabled_agents",
            "max_visible_rows",
            "hover_expand_ms",
            "collapse_delay_ms",
            "snap_distance",
            "hidden_sliver",
            "collapsed_size",
            "auto_hide_after_s",
            "edge_hide_enabled",
            "animations",
            "icon_style",
            "toast_on_degraded",
            "auto_dismiss_invalid",
            "auto_check_updates",
            "legacy_imported",
        ] {
            assert!(json.get(key).is_some(), "missing key {key} in {json}");
        }
        assert_eq!(json["enabled_agents"]["claude_code"], true);
        assert_eq!(json["notify_on_question"], true);
        assert_eq!(json["alert_sound"], true);
        assert_eq!(json["auto_show_on_question"], true);
    }

    #[test]
    fn settings_group_parsing_rejects_unknown_scopes() {
        assert_eq!(SettingsGroup::parse("all"), Some(SettingsGroup::All));
        assert_eq!(SettingsGroup::parse("attention"), Some(SettingsGroup::Attention));
        assert_eq!(SettingsGroup::parse("pet"), Some(SettingsGroup::Pet));
        assert_eq!(SettingsGroup::parse("update"), Some(SettingsGroup::Update));
        assert_eq!(SettingsGroup::parse("nope"), None);
    }

    #[test]
    fn attention_flags_default_true_and_accept_partial_json() {
        // An older settings.json has none of the keys: they must default to on.
        let s: Settings = serde_json::from_str(r#"{"retention_hours":24}"#).unwrap();
        assert!(s.notify_on_question && s.alert_sound && s.auto_show_on_question);
        // Partial JSON only overrides what it carries.
        let s: Settings = serde_json::from_str(r#"{"alert_sound":false}"#).unwrap();
        assert!(s.notify_on_question);
        assert!(!s.alert_sound);
        assert!(s.auto_show_on_question);
        // A non-boolean value is rejected, not silently coerced.
        assert!(serde_json::from_str::<Settings>(r#"{"alert_sound":"no"}"#).is_err());
    }

    #[test]
    fn auto_check_updates_defaults_true_and_accepts_partial_json() {
        // An older settings.json has no key: it must default to on.
        let s: Settings = serde_json::from_str(r#"{"retention_hours":24}"#).unwrap();
        assert!(s.auto_check_updates);
        // Partial JSON only overrides what it carries.
        let s: Settings = serde_json::from_str(r#"{"auto_check_updates":false}"#).unwrap();
        assert!(!s.auto_check_updates);
        // A non-boolean value is rejected, not silently coerced.
        assert!(serde_json::from_str::<Settings>(r#"{"auto_check_updates":"no"}"#).is_err());
    }

    #[test]
    fn update_group_reset_restores_only_the_update_flag() {
        let mut s = Settings {
            auto_check_updates: false,
            language: Language::Zh,
            notify_on_question: false,
            toast_on_degraded: false,
            ..Default::default()
        };
        s.reset_group(SettingsGroup::Update);
        assert!(s.auto_check_updates);
        assert_eq!(s.language, Language::Zh);
        assert!(!s.notify_on_question);
        assert!(!s.toast_on_degraded);
        // "all" still restores everything.
        s.auto_check_updates = false;
        s.reset_group(SettingsGroup::All);
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn attention_group_reset_restores_only_the_attention_flags() {
        let mut s = Settings {
            notify_on_question: false,
            alert_sound: false,
            auto_show_on_question: false,
            language: Language::Zh,
            new_badge: false,
            toast_on_degraded: false,
            ..Default::default()
        };
        s.reset_group(SettingsGroup::Attention);
        assert!(s.notify_on_question && s.alert_sound && s.auto_show_on_question);
        assert_eq!(s.language, Language::Zh);
        assert!(!s.new_badge);
        assert!(!s.toast_on_degraded);
    }
}
