//! Developer-only synthetic adapter used to prove that adding a new Agent does
//! not require touching the monitor or the UI main flow.
//!
//! It is **never** mixed with real records: it only runs when the environment
//! variable `AGENT_MONITOR_SIMULATED` is set to `1`, and every record carries
//! `origin: synthetic` plus the agent name `Simulated Agent`, which the UI
//! renders with an explicit `SIM` marker.

use super::{ScanCtx, Source};
use crate::model::*;

pub struct SimulatedSource {
    started_secs: u64,
}

impl SimulatedSource {
    pub fn new() -> Self {
        Self {
            started_secs: now_secs(),
        }
    }

    pub fn enabled() -> bool {
        std::env::var("AGENT_MONITOR_SIMULATED").map(|v| v == "1").unwrap_or(false)
    }
}

impl Source for SimulatedSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "simulated".into(),
            name: "Simulated Agent".into(),
            source: "in-process fixture (AGENT_MONITOR_SIMULATED=1)".into(),
            origin: DataOrigin::Synthetic,
            max_level: JumpLevel::L1,
            notes: "Extension acceptance only; never a real agent".into(),
        }
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let age = ctx.now_secs.saturating_sub(self.started_secs);
        let mk = |n: usize, status: TaskStatus, title: &str, secs_ago: u64| {
            let mut t = TaskRecord {
                id: format!("simulated:session-{n}"),
                agent_key: "simulated".into(),
                agent_name: "Simulated Agent".into(),
                host: "fixture".into(),
                session_id: Some(format!("session-{n}")),
                task_id: Some(format!("task-{n}")),
                title: title.into(),
                status,
                status_reason: "fixture event".into(),
                last_activity_secs: ctx.now_secs.saturating_sub(secs_ago),
                collected_at_secs: ctx.now_secs,
                summary: Some(format!("fixture age {age}s")),
                origin: DataOrigin::Synthetic,
                target: JumpTarget {
                    app_name: Some("Simulated Agent".into()),
                    ..Default::default()
                },
                target_validity: TargetValidity::Unconfirmed,
                invalid_reason: None,
                capability: JumpCapability {
                    max_level: JumpLevel::L1,
                    notes: "fixture has no real host".into(),
                    requires: vec![],
                },
                level: JumpLevel::L1,
                pinned: false,
            };
            t.refresh_level();
            t
        };

        vec![
            mk(1, TaskStatus::WaitingApproval, "Approve the release build", 4),
            mk(2, TaskStatus::Running, "Rewrite the parser", 1),
            mk(3, TaskStatus::Completed, "Add snapshot tests", 120),
        ]
    }
}
