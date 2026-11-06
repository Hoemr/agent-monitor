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

/// A failed turn only demands attention while it is recent: an interrupted or
/// long-finished failure must not keep the badge/tray alert alive forever.
pub const FAILED_ATTENTION_WINDOW_SECS: u64 = 30 * 60;

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

    /// Status-level class used for ordering/highlighting. For the badge and
    /// tray alert use [`TaskRecord::needs_attention_at`], which also applies the
    /// failure freshness/acknowledgement rules.
    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            TaskStatus::WaitingApproval | TaskStatus::WaitingInput | TaskStatus::Failed
        )
    }

    /// Whether this status means the agent is blocked waiting for the user
    /// (a "pending question", see `docs/ATTENTION.md`). Failures are not
    /// questions: they keep the normal attention rules.
    pub fn is_pending_question(self) -> bool {
        matches!(self, TaskStatus::WaitingInput | TaskStatus::WaitingApproval)
    }

    /// Whether this status should count as "needs the user" at `now_secs`.
    ///
    /// * `waiting_input` / `waiting_approval` always count: the agent is blocked.
    /// * `failed` counts only when it is recent and was not a deliberate
    ///   interruption/cancellation (`interrupted`, `aborted`, `cancelled`):
    ///   the user already decided to stop those turns.
    pub fn needs_attention_at(
        self,
        status_reason: &str,
        last_activity_secs: u64,
        now_secs: u64,
    ) -> bool {
        match self {
            TaskStatus::WaitingApproval | TaskStatus::WaitingInput => true,
            TaskStatus::Failed => {
                if is_cancelled_reason(status_reason) {
                    return false;
                }
                now_secs.saturating_sub(last_activity_secs) <= FAILED_ATTENTION_WINDOW_SECS
            }
            _ => false,
        }
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

/// Explicit end of the parent task/turn, never a tool or subagent result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionEvidence {
    TaskEnd,
    ClosedTurn,
    AssistantEndTurn,
    TaskIndex,
}

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
    /// Internal evidence checked by Monitor before publishing a completion.
    #[serde(skip)]
    pub completion: Option<CompletionEvidence>,
    /// Stable source identity of the final answer; unrelated activity must not change it.
    #[serde(default)]
    pub completion_id: Option<String>,
    /// Evidence for the status: source event name or short reason ("turn/start").
    pub status_reason: String,
    /// When activity actually happened (source timestamp), not when we read it.
    pub last_activity_secs: u64,
    /// When this record was last collected.
    pub collected_at_secs: u64,
    /// Optional, short, non-sensitive progress line.
    pub summary: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub origin: DataOrigin,
    pub target: JumpTarget,
    pub target_validity: TargetValidity,
    pub invalid_reason: Option<String>,
    pub capability: JumpCapability,
    /// Effective level the UI may advertise (min(capability, target validity)).
    pub level: JumpLevel,
    pub pinned: bool,
}

/// Reasons that represent a deliberate stop rather than an error the user must
/// act on. Adapters report e.g. `thread_turns.status = interrupted` (Codex) and
/// `turn aborted` (DSH).
fn is_cancelled_reason(status_reason: &str) -> bool {
    let reason = status_reason.to_ascii_lowercase();
    reason.contains("interrupt") || reason.contains("abort") || reason.contains("cancel")
}

impl TaskRecord {
    pub fn enforce_completion(&mut self) {
        if self.status == TaskStatus::Completed && self.completion.is_none() {
            self.status = TaskStatus::Unknown;
            self.status_reason = format!("final completion unconfirmed: {}", self.status_reason);
        }
        if self.status != TaskStatus::Completed {
            self.completion = None;
            self.completion_id = None;
        }
    }

    /// Status-class helper (see [`TaskStatus::needs_attention`]).
    pub fn needs_attention(&self) -> bool {
        self.status.needs_attention()
    }

    /// Aggregation used by the badge, header count and tray alert.
    pub fn needs_attention_at(&self, now_secs: u64) -> bool {
        self.status
            .needs_attention_at(&self.status_reason, self.last_activity_secs, now_secs)
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
    /// User asked to hide while `edge_hide_enabled` is off: the window is
    /// ordered out (no off-screen geometry) and recalled from the tray.
    TrayHidden,
}

impl PetMode {
    /// True when the window is deliberately not on screen (either parked at the
    /// edge or fully hidden). Used for the worker's 30 s slow path.
    pub fn is_offscreen(self) -> bool {
        matches!(self, PetMode::EdgeHidden | PetMode::TrayHidden)
    }
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

/// Persisted pet placement/hidden flag (`pet.json`).
///
/// Behavioural preferences (hover/collapse timing, row count, snap distance,
/// sliver width) moved to [`crate::settings::Settings`] so there is exactly one
/// user-facing settings source. Older `pet.json` files that still carry those
/// keys parse fine: serde ignores unknown fields, and missing fields default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PetPrefs {
    pub hidden: bool,
    pub position: PetPosition,
}

// ── Snapshot sent to the UI ─────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub tasks: Vec<TaskRecord>,
    /// Local display names; source titles remain intact for navigation.
    #[serde(default)]
    pub custom_names: std::collections::BTreeMap<String, String>,
    pub collected_at_secs: u64,
    /// Count of tasks that need user attention (precomputed for the collapsed badge).
    pub attention_count: usize,
    /// Count of tasks blocked on a question (`waiting_input` + `waiting_approval`).
    /// Always a subset of `attention_count` (failures are excluded here); the pet
    /// uses it for the amber "waiting for you" chip even before an event arrives.
    pub question_count: usize,
    pub agents: Vec<AgentDescriptor>,
}

impl Snapshot {
    pub fn new(tasks: Vec<TaskRecord>, agents: Vec<AgentDescriptor>) -> Self {
        let mut snapshot = Self {
            tasks,
            custom_names: Default::default(),
            collected_at_secs: now_secs(),
            attention_count: 0,
            question_count: 0,
            agents,
        };
        snapshot.recount();
        snapshot
    }

    /// Recompute the derived counts after `tasks` changed in place (dismissal).
    /// Both counters are derived from the task list, so they are deliberately
    /// not part of [`Snapshot::render_hash`]: a status change already moves it.
    pub fn recount(&mut self) {
        let now = now_secs();
        self.attention_count = self.tasks.iter().filter(|t| t.needs_attention_at(now)).count();
        self.question_count = self
            .tasks
            .iter()
            .filter(|t| t.status.is_pending_question())
            .count();
    }

    /// Stable hash of everything that affects rendering, used to suppress
    /// duplicate UI updates. `collected_at_secs` is deliberately excluded.
    pub fn render_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.attention_count.hash(&mut h);
        self.custom_names.hash(&mut h);
        for t in &self.tasks {
            t.id.hash(&mut h);
            t.status.key().hash(&mut h);
            t.title.hash(&mut h);
            t.summary.hash(&mut h);
            t.model.hash(&mut h);
            t.pinned.hash(&mut h);
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
            completion_id: Some("100".into()),
            completion: (status == TaskStatus::Completed).then_some(CompletionEvidence::TaskEnd),
            status_reason: "task_started".into(),
            last_activity_secs: 1,
            collected_at_secs: 2,
            summary: None,
            model: None,
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
    fn attention_ignores_old_and_cancelled_failures() {
        let now = 1_000_000;
        // A deliberately interrupted turn is never "pending", even if just now.
        assert!(!TaskStatus::Failed.needs_attention_at(
            "thread_turns.status = interrupted",
            now - 5,
            now
        ));
        assert!(!TaskStatus::Failed.needs_attention_at("turn aborted", now - 5, now));
        assert!(!TaskStatus::Failed.needs_attention_at("cancelled by user", now - 5, now));
        // A real failure counts while recent...
        assert!(TaskStatus::Failed.needs_attention_at("panic", now - 60, now));
        // ...but stops demanding attention once stale.
        assert!(TaskStatus::Failed.needs_attention_at(
            "panic",
            now - FAILED_ATTENTION_WINDOW_SECS,
            now
        ));
        assert!(!TaskStatus::Failed.needs_attention_at(
            "panic",
            now - FAILED_ATTENTION_WINDOW_SECS - 1,
            now
        ));
        // Waiting states block the agent and always count, regardless of age.
        for status in [TaskStatus::WaitingInput, TaskStatus::WaitingApproval] {
            assert!(status.needs_attention_at("", 0, now));
        }
        assert!(!TaskStatus::Completed.needs_attention_at("", now, now));
    }

    #[test]
    fn snapshot_attention_count_uses_freshness_rules() {
        let now = now_secs();
        let mut old_failed = task("old-failed", TaskStatus::Failed, "old");
        old_failed.last_activity_secs = now.saturating_sub(FAILED_ATTENTION_WINDOW_SECS + 10);
        old_failed.status_reason = "panic".into();
        let mut cancelled = task("cancelled", TaskStatus::Failed, "stopped");
        cancelled.last_activity_secs = now;
        cancelled.status_reason = "thread_turns.status = interrupted".into();
        let mut waiting = task("waiting", TaskStatus::WaitingInput, "waiting");
        waiting.last_activity_secs = now.saturating_sub(24 * 3600);
        let mut recent_failed = task("recent", TaskStatus::Failed, "recent");
        recent_failed.last_activity_secs = now;
        let snapshot = Snapshot::new(vec![old_failed, cancelled, waiting, recent_failed], vec![]);
        assert_eq!(snapshot.attention_count, 2, "only waiting + recent failure");
    }

    #[test]
    fn question_count_only_counts_blocking_questions() {
        let now = now_secs();
        let mut waiting_input = task("q-input", TaskStatus::WaitingInput, "input");
        waiting_input.last_activity_secs = now;
        let mut waiting_approval = task("q-approval", TaskStatus::WaitingApproval, "approval");
        waiting_approval.last_activity_secs = now.saturating_sub(3600); // age never matters
        let mut failed = task("failed", TaskStatus::Failed, "failed");
        failed.last_activity_secs = now;
        let mut running = task("running", TaskStatus::Running, "running");
        running.last_activity_secs = now;
        let mut completed = task("completed", TaskStatus::Completed, "completed");
        completed.last_activity_secs = now;

        let snapshot = Snapshot::new(
            vec![waiting_input, waiting_approval, failed, running, completed],
            vec![],
        );
        assert_eq!(snapshot.question_count, 2);
        // Failures still need attention but are not questions.
        assert_eq!(snapshot.attention_count, 3);

        // Derived counter follows in-place dismissal-style changes.
        let mut mutated = snapshot;
        mutated.tasks.retain(|t| t.id != "q-input");
        mutated.recount();
        assert_eq!(mutated.question_count, 1);
        assert_eq!(mutated.attention_count, 2);
    }

    #[test]
    fn pending_question_status_helper_matches_the_contract() {
        assert!(TaskStatus::WaitingInput.is_pending_question());
        assert!(TaskStatus::WaitingApproval.is_pending_question());
        for status in [
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Idle,
            TaskStatus::Unknown,
        ] {
            assert!(!status.is_pending_question());
        }
    }

    #[test]
    fn render_hash_ignores_derived_question_count() {
        // The counter is derived from statuses (already hashed); changing it via
        // recount must not change the hash on its own.
        let mut a = Snapshot::new(vec![task("a", TaskStatus::Running, "t")], vec![]);
        let before = a.render_hash();
        a.question_count = 99;
        assert_eq!(a.render_hash(), before);
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
    fn render_hash_distinguishes_model_and_pin_updates() {
        let original = task("a", TaskStatus::Running, "t");
        let base = Snapshot::new(vec![original.clone()], vec![]).render_hash();
        let mut updated = original.clone();
        updated.model = Some("gpt-6.1-sol".into());
        assert_ne!(base, Snapshot::new(vec![updated], vec![]).render_hash());
        let mut updated = original;
        updated.pinned = true;
        assert_ne!(base, Snapshot::new(vec![updated], vec![]).render_hash());
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
