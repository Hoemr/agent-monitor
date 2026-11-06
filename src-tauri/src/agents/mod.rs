//! Agent adapters.
//!
//! Every adapter converts one real local data source into the unified
//! [`TaskRecord`] model. The monitor knows nothing about Codex/DSH/ZCode
//! specifics; the UI never branches on agent names.

use crate::model::{AgentDescriptor, TaskRecord, TaskStatus};
use crate::settings::EnabledAgents;
use std::path::PathBuf;
use std::sync::Arc;

pub mod claude_code;
pub mod antigravity;
pub mod codex;
pub mod dsh;
pub mod simulated;
pub mod zcode;

/// Explicit interactive tool names, allowing namespace-qualified spellings.
pub fn is_question_tool(name: &str) -> bool {
    let name = name.rsplit('.').next().unwrap_or(name);
    let name = name.rsplit("__").next().unwrap_or(name);
    matches!(name.to_ascii_lowercase().as_str(),
        "request_user_input" | "request_user_input_async" | "askuserquestion" | "ask_user_question" | "ask_user")
}

/// Cheap snapshot of the process table, refreshed at most every 2 s.
/// Adapters use it to decide whether a host application is still alive
/// (process presence is evidence, not proof that a task is running).
#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub command: String,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessTable {
    pub entries: Vec<ProcInfo>,
    pub sampled_at_ms: u64,
}

impl ProcessTable {
    pub fn sample() -> Self {
        #[cfg(target_os = "windows")]
        return Self {
            entries: crate::windows::processes(),
            sampled_at_ms: crate::model::now_millis(),
        };
        #[cfg(not(target_os = "windows"))]
        {
        let mut entries = Vec::new();
        if let Ok(out) = std::process::Command::new("ps")
            .args(["-axo", "pid=,command="])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let line = line.trim_start();
                let mut it = line.splitn(2, char::is_whitespace);
                let pid = it.next().and_then(|p| p.trim().parse::<u32>().ok());
                let command = it.next().unwrap_or("").to_string();
                if let Some(pid) = pid {
                    entries.push(ProcInfo { pid, command });
                }
            }
        }
        Self {
            entries,
            sampled_at_ms: crate::model::now_millis(),
        }
        }
    }

    pub fn find(&self, needle: &str) -> Option<&ProcInfo> {
        self.entries.iter().find(|p| p.command.contains(needle))
    }

    pub fn any(&self, needle: &str) -> bool {
        self.find(needle).is_some()
    }

    /// Windows process snapshots contain executable names, without command lines.
    pub fn has_executable(&self, name: &str) -> bool {
        self.entries.iter().any(|p| {
            p.command.rsplit(['/', '\\']).next().unwrap_or("")
                .to_ascii_lowercase().trim_end_matches(".exe").eq_ignore_ascii_case(name)
        })
    }

    pub fn pids(&self, needle: &str) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|p| p.command.contains(needle))
            .map(|p| p.pid)
            .collect()
    }
}

/// Per-scan context shared by all adapters.
///
/// `retention_secs` is injected from [`crate::settings::Settings::retention_secs`]
/// on every scan; adapters must never hard-code their own window.
#[derive(Clone, Debug)]
pub struct ScanCtx {
    pub now_secs: u64,
    pub processes: Arc<ProcessTable>,
    pub retention_secs: u64,
    pub pinned_ids: Arc<std::collections::HashSet<String>>,
}

impl ScanCtx {
    pub fn new(now_secs: u64, processes: Arc<ProcessTable>, retention_secs: u64) -> Self {
        Self {
            now_secs,
            processes,
            retention_secs,
            pinned_ids: Arc::new(std::collections::HashSet::new()),
        }
    }
}

/// One adapter = one data source. Implementations must be cheap when nothing
/// changed (mtime/db checks) and must never spawn per-task timers.
pub trait Source: Send {
    fn descriptor(&self) -> AgentDescriptor;
    /// Completed records must include `CompletionEvidence` from the current
    /// parent task/turn. Monitor rejects unsupported completion claims.
    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord>;

    /// Force the next `scan` to re-read everything (used after the user clicks
    /// "refresh"; normal scans are incremental).
    fn invalidate(&mut self) {}
}

/// Builds the production registry from the user's per-agent switches.
/// Order defines the UI's tie-break order and must stay stable.
pub fn build_registry(enabled: &EnabledAgents) -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = Vec::new();
    if enabled.claude_code {
        sources.push(Box::new(claude_code::ClaudeCodeSource::new()));
    }
    if enabled.codex {
        sources.push(Box::new(codex::CodexSource::new()));
    }
    if enabled.dsh {
        sources.push(Box::new(dsh::DshSource::new()));
    }
    if enabled.zcode {
        sources.push(Box::new(zcode::ZCodeSource::new()));
    }
    if enabled.antigravity {
        sources.push(Box::new(antigravity::AntigravitySource::new()));
    }
    sources
}

/// Shared retention rule: work the user may still be waiting on is always kept,
/// no matter how old its last event is. Everything finished (completed, failed,
/// idle, unknown) is shown only while `now - activity <= retention`.
///
/// The boundary is inclusive so "retention = 6 h" keeps a task whose last
/// activity was exactly 6 h ago.
pub fn retention_keeps(
    status: TaskStatus,
    activity_secs: u64,
    now_secs: u64,
    retention_secs: u64,
) -> bool {
    match status {
        // Active states must never disappear because of an age threshold: the
        // user may be about to act on exactly those rows.
        TaskStatus::Running | TaskStatus::WaitingInput | TaskStatus::WaitingApproval => true,
        _ => activity_secs >= now_secs.saturating_sub(retention_secs),
    }
}

// ── Shared helpers ──────────────────────────────────────────────

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

pub fn file_mtime_secs(path: &std::path::Path) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        })
        .unwrap_or(0)
}

/// Cached directory mtime helper (some filesystems update dir mtime on append).
pub fn dir_mtime_secs(path: &std::path::Path) -> u64 {
    file_mtime_secs(path)
}

pub fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Pinned conversations first, then newest activity within each group.
/// Status only breaks ties; waiting/failed tasks keep their attention surfaces.
pub fn sort_tasks(tasks: &mut [TaskRecord]) {
    tasks.sort_by(|a, b| {
        b.pinned.cmp(&a.pinned)
            .then_with(|| b.last_activity_secs.cmp(&a.last_activity_secs))
            .then_with(|| a.status.priority().cmp(&b.status.priority()))
            .then_with(|| a.agent_key.cmp(&b.agent_key))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_executable_names_match_without_substring_collisions() {
        let table = ProcessTable { entries: vec![
            ProcInfo { pid: 1, command: r"C:\Program Files\Microsoft VS Code\Code.EXE".into() },
            ProcInfo { pid: 2, command: "codex.exe".into() },
        ], sampled_at_ms: 0 };
        assert!(table.has_executable("Code"));
        assert!(table.has_executable("codex"));
        assert!(!table.has_executable("ode"));
        assert!(!table.has_executable("claude"));
    }

    #[test]
    fn question_tools_are_explicit_and_allow_namespaces() {
        for name in ["AskUserQuestion", "functions.request_user_input", "mcp__agent__ask_user_question", "request_user_input_async"] {
            assert!(is_question_tool(name), "{name}");
        }
        for name in ["Read", "Agent", "ask_database", "finish_question"] {
            assert!(!is_question_tool(name), "{name}");
        }
    }
    use crate::model::*;

    fn rec(id: &str, status: TaskStatus, activity: u64) -> TaskRecord {
        TaskRecord {
            id: id.into(),
            agent_key: "a".into(),
            agent_name: "A".into(),
            host: "desktop".into(),
            session_id: Some(id.into()),
            task_id: None,
            title: id.into(),
            status,
            completion_id: Some("100".into()),
            completion: (status == TaskStatus::Completed).then_some(CompletionEvidence::TaskEnd),
            status_reason: String::new(),
            last_activity_secs: activity,
            collected_at_secs: activity,
            summary: None,
            model: None,
            origin: DataOrigin::Index,
            target: JumpTarget::default(),
            target_validity: TargetValidity::Valid,
            invalid_reason: None,
            capability: JumpCapability::default(),
            level: JumpLevel::L1,
            pinned: false,
        }
    }

    #[test]
    fn newest_activity_comes_first_regardless_of_status() {
        let mut tasks = vec![
            rec("old-running", TaskStatus::Running, 10),
            rec("new-completed", TaskStatus::Completed, 900),
            rec("mid-waiting", TaskStatus::WaitingApproval, 500),
            rec("oldest-failed", TaskStatus::Failed, 5),
        ];
        sort_tasks(&mut tasks);
        let order: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, vec!["new-completed", "mid-waiting", "old-running", "oldest-failed"]);
    }

    #[test]
    fn equal_activity_prefers_attention_status() {
        let mut tasks = vec![
            rec("completed", TaskStatus::Completed, 100),
            rec("waiting", TaskStatus::WaitingApproval, 100),
            rec("running", TaskStatus::Running, 100),
        ];
        sort_tasks(&mut tasks);
        let order: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, vec!["waiting", "running", "completed"]);
    }

    #[test]
    fn pinned_group_precedes_recent_tasks_and_is_itself_newest_first() {
        let mut older = rec("pinned-old", TaskStatus::Completed, 1);
        let mut newer = rec("pinned-new", TaskStatus::Completed, 2);
        older.pinned = true;
        newer.pinned = true;
        let mut tasks = vec![rec("recent", TaskStatus::Running, 999), older, newer];
        sort_tasks(&mut tasks);
        assert_eq!(tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["pinned-new", "pinned-old", "recent"]);
    }

    #[test]
    fn sort_is_deterministic_for_equal_status_and_time() {
        let mut a = vec![rec("b", TaskStatus::Running, 5), rec("a", TaskStatus::Running, 5)];
        sort_tasks(&mut a);
        let mut b = vec![rec("a", TaskStatus::Running, 5), rec("b", TaskStatus::Running, 5)];
        sort_tasks(&mut b);
        assert_eq!(
            a.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            b.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn registry_reflects_enabled_agents_and_keeps_stable_order() {
        let keys = |enabled: &EnabledAgents| {
            build_registry(enabled)
                .iter()
                .map(|s| s.descriptor().key)
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&EnabledAgents::default()), vec!["claude-code", "codex", "dsh", "zcode", "antigravity"]);
        assert_eq!(
            keys(&EnabledAgents {
                codex: false,
                dsh: false,
                ..Default::default()
            }),
            vec!["claude-code", "zcode", "antigravity"]
        );
        assert_eq!(
            keys(&EnabledAgents {
                codex: false,
                claude_code: false,
                ..Default::default()
            }),
            vec!["dsh", "zcode", "antigravity"]
        );
        assert!(build_registry(&EnabledAgents {
            codex: false,
            dsh: false,
            zcode: false,
            claude_code: false,
            antigravity: false,
        })
        .is_empty());
    }

    #[test]
    fn scan_ctx_carries_the_injected_retention_window() {
        let ctx = ScanCtx::new(1_000, Arc::new(ProcessTable::default()), 42);
        assert_eq!(ctx.now_secs, 1_000);
        assert_eq!(ctx.retention_secs, 42);
        assert!(ctx.processes.entries.is_empty());
    }

    #[test]
    fn retention_always_keeps_active_statuses_regardless_of_age() {
        let now = 1_000_000;
        for status in [
            TaskStatus::Running,
            TaskStatus::WaitingInput,
            TaskStatus::WaitingApproval,
        ] {
            assert!(retention_keeps(status, 0, now, 3_600), "{status:?} must be exempt");
            assert!(
                retention_keeps(status, now - 10 * 3_600, now, 3_600),
                "{status:?} must be exempt even far outside the window"
            );
        }
    }

    #[test]
    fn retention_drops_finished_work_outside_the_window_inclusive_boundary() {
        let now = 1_000_000;
        let window = 6 * 3_600;
        for status in [
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Idle,
            TaskStatus::Unknown,
        ] {
            assert!(
                retention_keeps(status, now - window, now, window),
                "{status:?} at the boundary stays"
            );
            assert!(retention_keeps(status, now - 60, now, window));
            assert!(
                !retention_keeps(status, now - window - 1, now, window),
                "{status:?} one second past the window is dropped"
            );
            assert!(!retention_keeps(status, 0, now, 3_600));
        }
        // Very large windows must not overflow or flip to "keep nothing".
        assert!(retention_keeps(TaskStatus::Completed, 0, now, u64::MAX));
    }
}
