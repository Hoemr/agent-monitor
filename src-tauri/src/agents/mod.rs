//! Agent adapters.
//!
//! Every adapter converts one real local data source into the unified
//! [`TaskRecord`] model. The monitor knows nothing about Codex/DSH/ZCode
//! specifics; the UI never branches on agent names.

use crate::model::{AgentDescriptor, TaskRecord};
use std::path::PathBuf;
use std::sync::Arc;

pub mod claude_code;
pub mod codex;
pub mod dsh;
pub mod simulated;
pub mod zcode;

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

    pub fn find(&self, needle: &str) -> Option<&ProcInfo> {
        self.entries.iter().find(|p| p.command.contains(needle))
    }

    pub fn any(&self, needle: &str) -> bool {
        self.find(needle).is_some()
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
#[derive(Clone, Debug)]
pub struct ScanCtx {
    pub now_secs: u64,
    pub processes: Arc<ProcessTable>,
}

impl ScanCtx {
    pub fn new(now_secs: u64, processes: Arc<ProcessTable>) -> Self {
        Self {
            now_secs,
            processes,
        }
    }
}

/// One adapter = one data source. Implementations must be cheap when nothing
/// changed (mtime/db checks) and must never spawn per-task timers.
pub trait Source: Send {
    fn descriptor(&self) -> AgentDescriptor;
    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord>;

    /// Force the next `scan` to re-read everything (used after the user clicks
    /// "refresh"; normal scans are incremental).
    fn invalidate(&mut self) {}
}

/// Builds the production registry. Order defines the UI's tie-break order.
pub fn build_registry() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(claude_code::ClaudeCodeSource::new()),
        Box::new(codex::CodexSource::new()),
        Box::new(dsh::DshSource::new()),
        Box::new(zcode::ZCodeSource::new()),
    ]
}

// ── Shared helpers ──────────────────────────────────────────────

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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

/// Sort keys are stable ids so the UI order does not jitter between scans.
pub fn sort_tasks(tasks: &mut [TaskRecord]) {
    tasks.sort_by(|a, b| {
        a.status
            .priority()
            .cmp(&b.status.priority())
            .then_with(|| a.agent_key.cmp(&b.agent_key))
            .then_with(|| b.last_activity_secs.cmp(&a.last_activity_secs))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
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
            status_reason: String::new(),
            last_activity_secs: activity,
            collected_at_secs: activity,
            summary: None,
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
    fn attention_first_then_recent_activity() {
        let mut tasks = vec![
            rec("completed", TaskStatus::Completed, 900),
            rec("running", TaskStatus::Running, 100),
            rec("waiting", TaskStatus::WaitingApproval, 50),
            rec("failed", TaskStatus::Failed, 10),
        ];
        sort_tasks(&mut tasks);
        let order: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, vec!["waiting", "failed", "running", "completed"]);
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
}
