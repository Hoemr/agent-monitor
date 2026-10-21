//! Unified task model shared by every agent adapter, the monitor, the jump
//! executor and the desktop-pet UI.
//!
//! Rules encoded here (see docs/PHASE0.md §6):
//! * the record id never comes from the title;
//! * "last activity" and "collected at" are separate timestamps;
//! * a task's status and its target's validity are independent;
//! * the declared jump level can never exceed what the adapter can support.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Task status ─────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    WaitingInput,
    WaitingApproval,
    Completed,
    Failed,
    Idle,
    Unknown,
}

impl TaskStatus {
    /// Stable i18n key consumed by the UI (never a hard-coded English string).
    pub fn key(self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::WaitingInput => "waiting_input",
            TaskStatus::WaitingApproval => "waiting_approval",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Idle => "idle",
            TaskStatus::Unknown => "unknown",
        }
    }

    /// Ordering used by the task list: things the user must act on come first.
    pub fn priority(self) -> u8 {
        match self {
            TaskStatus::WaitingApproval => 0,
            TaskStatus::WaitingInput => 1,
            TaskStatus::Failed => 2,
            TaskStatus::Running => 3,
            TaskStatus::Unknown => 4,
            TaskStatus::Idle => 5,
            TaskStatus::Completed => 6,
        }
    }

    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            TaskStatus::WaitingApproval | TaskStatus::WaitingInput | TaskStatus::Failed
        )
    }
}

// ── Jump model ──────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum JumpLevel {
    L0,
    L1,
    L2,
    L3,
}

impl JumpLevel {
    pub fn code(self) -> &'static str {
        match self {
            JumpLevel::L0 => "L0",
            JumpLevel::L1 => "L1",
            JumpLevel::L2 => "L2",
            JumpLevel::L3 => "L3",
        }
    }

    pub fn min(self, other: JumpLevel) -> JumpLevel {
        if self <= other {
            self
        } else {
            other
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetValidity {
    Valid,
    Invalid,
    Unconfirmed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Accessibility,
    Automation,
}

impl PermissionKind {
    pub fn key(self) -> &'static str {
        match self {
            PermissionKind::Accessibility => "accessibility",
            PermissionKind::Automation => "automation",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataOrigin {
    /// A live event stream / watcher with semantics (JSONL events).
    Event,
    /// A local index database written by the agent itself.
    Index,
    /// A local log/cache file.
    Cache,
    /// Developer-only synthetic adapter (never mixed with real records).
    Synthetic,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JumpTarget {
    /// macOS application name as shown to the user.
    pub app_name: Option<String>,
    /// Bundle identifier used with `open -b`.
    pub bundle_id: Option<String>,
    /// Fully-formed, verified deep link (only set when the route is known).
    pub deep_link: Option<String>,
    /// Exact window title for AX matching (verified by the adapter when possible).
    pub window_title: Option<String>,
    /// Project/workspace directory (used for VSCode / Terminal matching).
    pub workspace_path: Option<String>,
    /// Terminal host application name, e.g. "Terminal".
    pub terminal_host: Option<String>,
    /// Controlling TTY of the agent process, e.g. "/dev/ttys004".
    pub terminal_tty: Option<String>,
    /// Opaque session/tab reference meaningful to the host.
    pub host_ref: Option<String>,
    /// Process id when the adapter can identify the exact agent process.
    pub process_id: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JumpCapability {
    /// Highest level this record can *reliably* reach right now.
    pub max_level: JumpLevel,
    /// Human readable explanation of the limit ("window title matched, no session API").
    pub notes: String,
    /// Permissions that must be granted for `max_level` to actually work.
    pub requires: Vec<PermissionKind>,
}

impl Default for JumpCapability {
    fn default() -> Self {
        Self {
            max_level: JumpLevel::L1,
            notes: String::new(),
            requires: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JumpOutcomeKind {
    Success,
    Degraded,
    Failed,
    Unconfirmed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JumpOutcome {
    pub task_id: String,
    pub kind: JumpOutcomeKind,
    pub requested: JumpLevel,
    pub achieved: JumpLevel,
    /// Short, user-facing reason (i18n key + optional detail).
    pub reason_key: String,
    pub detail: Option<String>,
    /// What the user can do next, e.g. "打开系统设置 → 辅助功能".
    pub next_step: Option<String>,
    pub permission_needed: Option<PermissionKind>,
}

impl JumpOutcome {
    pub fn failed(task_id: &str, requested: JumpLevel, reason: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            kind: JumpOutcomeKind::Failed,
            requested,
            achieved: JumpLevel::L0,
            reason_key: reason.to_string(),
            detail: None,
            next_step: None,
            permission_needed: None,
        }
    }
}

// ── Task record ─────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Stable identity: `agent:session-id` (never derived from the title).
    pub id: String,
    /// Stable adapter key, e.g. "codex", "dsh", "zcode", "claude-code".
    pub agent_key: String,
    /// Display name provided by the adapter (UI must not hard-code agent names).
    pub agent_name: String,
    /// Host application / embedding, e.g. "desktop", "vscode", "cli", "terminal".
    pub host: String,
    /// Session identity exactly as the data source provides it (may be empty).
    pub session_id: Option<String>,
    /// Task/turn identity when the source distinguishes it from the session.
    pub task_id: Option<String>,
    pub title: String,
    pub status: TaskStatus,
    /// Evidence for the status: source event name or short reason ("turn/start").
    pub status_reason: String,
    /// When activity actually happened (source timestamp), not when we read it.
    pub last_activity_secs: u64,
    /// When this record was last collected.
    pub collected_at_secs: u64,
    /// Optional, short, non-sensitive progress line.
    pub summary: Option<String>,
    pub origin: DataOrigin,
    pub target: JumpTarget,
    pub target_validity: TargetValidity,
    pub invalid_reason: Option<String>,
    pub capability: JumpCapability,
    /// Effective level the UI may advertise (min(capability, target validity)).
    pub level: JumpLevel,
    pub pinned: bool,
}

impl TaskRecord {
    pub fn needs_attention(&self) -> bool {
        self.status.needs_attention()
    }

    /// Recompute the advertised jump level from capability + target validity.
    pub fn refresh_level(&mut self) {
        self.level = match self.target_validity {
            TargetValidity::Valid => self.capability.max_level,
            TargetValidity::Unconfirmed => self.capability.max_level.min(JumpLevel::L2),
            TargetValidity::Invalid => JumpLevel::L0,
        };
    }
}

/// Adapter self-description used by the capability matrix and the UI legend.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub key: String,
    pub name: String,
    /// Where the data comes from (path or db, for docs/diagnostics).
    pub source: String,
    pub origin: DataOrigin,
    /// Highest level this adapter can ever declare (verified level in docs).
    pub max_level: JumpLevel,
    pub notes: String,
}

// ── Desktop pet model ───────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PetMode {
    Collapsed,
    Expanded,
    Dragging,
    EdgeHidden,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PetState {
    pub mode: PetMode,
    pub edge: Edge,
    pub hidden: bool,
}

impl Default for PetState {
    fn default() -> Self {
        Self {
            mode: PetMode::Collapsed,
            edge: Edge::None,
            hidden: false,
        }
    }
}

/// Persisted pet placement. Screen identity is the macOS display UUID/name;
/// the position is stored as a fraction of the visible frame so resolution and
/// Dock changes keep it on screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PetPosition {
    pub screen_key: String,
    /// Fraction of the visible frame (0..1) of the window's top-left corner.
    pub rel_x: f64,
    pub rel_y: f64,
    pub edge: Edge,
}

impl Default for PetPosition {
    fn default() -> Self {
        Self {
            screen_key: String::new(),
            rel_x: 1.0,
            rel_y: 0.45,
            edge: Edge::Right,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PetPrefs {
    pub hidden: bool,
    pub position: PetPosition,
    pub hover_expand_ms: u64,
    pub collapse_delay_ms: u64,
    pub max_visible_rows: u32,
}

impl Default for PetPrefs {
    fn default() -> Self {
        Self {
            hidden: false,
            position: PetPosition::default(),
            hover_expand_ms: 150,
            collapse_delay_ms: 800,
            max_visible_rows: 5,
        }
    }
}

// ── Snapshot sent to the UI ─────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub tasks: Vec<TaskRecord>,
    pub collected_at_secs: u64,
    /// Count of tasks that need user attention (precomputed for the collapsed badge).
    pub attention_count: usize,
    pub agents: Vec<AgentDescriptor>,
}

impl Snapshot {
    pub fn new(tasks: Vec<TaskRecord>, agents: Vec<AgentDescriptor>) -> Self {
        let attention_count = tasks.iter().filter(|t| t.needs_attention()).count();
        Self {
            tasks,
            collected_at_secs: now_secs(),
            attention_count,
            agents,
        }
    }

    /// Stable hash of everything that affects rendering, used to suppress
    /// duplicate UI updates. `collected_at_secs` is deliberately excluded.
    pub fn render_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.attention_count.hash(&mut h);
        for t in &self.tasks {
            t.id.hash(&mut h);
            t.status.key().hash(&mut h);
            t.title.hash(&mut h);
            t.summary.hash(&mut h);
            t.last_activity_secs.hash(&mut h);
            t.level.code().hash(&mut h);
            t.target_validity.hash(&mut h);
            t.invalid_reason.hash(&mut h);
            t.status_reason.hash(&mut h);
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus, title: &str) -> TaskRecord {
        TaskRecord {
            id: id.into(),
            agent_key: "codex".into(),
            agent_name: "Codex".into(),
            host: "desktop".into(),
            session_id: Some("s1".into()),
            task_id: None,
            title: title.into(),
            status,
            status_reason: "task_started".into(),
            last_activity_secs: 1,
            collected_at_secs: 2,
            summary: None,
            origin: DataOrigin::Event,
            target: JumpTarget::default(),
            target_validity: TargetValidity::Valid,
            invalid_reason: None,
            capability: JumpCapability::default(),
            level: JumpLevel::L1,
            pinned: false,
        }
    }

    #[test]
    fn attention_priority_is_stable() {
        assert!(TaskStatus::WaitingApproval.priority() < TaskStatus::Running.priority());
        assert!(TaskStatus::Failed.needs_attention());
        assert!(!TaskStatus::Completed.needs_attention());
        assert!(!TaskStatus::Idle.needs_attention());
    }

    #[test]
    fn render_hash_ignores_collection_time_but_not_status() {
        let a = Snapshot::new(vec![task("a", TaskStatus::Running, "t")], vec![]);
        let mut b = Snapshot::new(vec![task("a", TaskStatus::Running, "t")], vec![]);
        b.collected_at_secs += 100;
        assert_eq!(a.render_hash(), b.render_hash());

        let c = Snapshot::new(vec![task("a", TaskStatus::Completed, "t")], vec![]);
        assert_ne!(a.render_hash(), c.render_hash());
    }

    #[test]
    fn render_hash_distinguishes_activity_time() {
        let a = Snapshot::new(vec![task("a", TaskStatus::Running, "t")], vec![]);
        let mut t = task("a", TaskStatus::Running, "t");
        t.last_activity_secs = 99;
        let b = Snapshot::new(vec![t], vec![]);
        assert_ne!(a.render_hash(), b.render_hash());
    }

    #[test]
    fn invalid_target_forces_level_zero() {
        let mut t = task("a", TaskStatus::Completed, "t");
        t.capability.max_level = JumpLevel::L3;
        t.target_validity = TargetValidity::Invalid;
        t.refresh_level();
        assert_eq!(t.level, JumpLevel::L0);
    }

    #[test]
    fn unconfirmed_target_caps_level_at_two() {
        let mut t = task("a", TaskStatus::Running, "t");
        t.capability.max_level = JumpLevel::L3;
        t.target_validity = TargetValidity::Unconfirmed;
        t.refresh_level();
        assert_eq!(t.level, JumpLevel::L2);
    }

    #[test]
    fn level_min_is_not_greater_than_arguments() {
        assert_eq!(JumpLevel::L3.min(JumpLevel::L2), JumpLevel::L2);
        assert_eq!(JumpLevel::L0.min(JumpLevel::L3), JumpLevel::L0);
    }
}
