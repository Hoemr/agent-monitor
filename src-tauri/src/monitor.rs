//! Collects records from every adapter, keeps a single unified snapshot and
//! suppresses duplicate UI updates.

use crate::agents::{self, ProcessTable, ScanCtx, Source};
use crate::model::{now_millis, AgentDescriptor, Snapshot, TaskRecord, TargetValidity};
use std::collections::HashMap;
use std::sync::Arc;

pub struct Monitor {
    sources: Vec<Box<dyn Source>>,
    processes: Arc<ProcessTable>,
    snapshot: Snapshot,
    last_hash: u64,
    /// id -> last_activity_secs at the moment the user dismissed it.
    dismissed: HashMap<String, u64>,
}

impl Monitor {
    pub fn new() -> Self {
        // Measurement aid: AGENT_MONITOR_EMPTY=1 gives a truly empty snapshot so
        // the "no tasks, idle" CPU budget can be measured without mocking data.
        let mut sources = if std::env::var("AGENT_MONITOR_EMPTY").as_deref() == Ok("1") {
            Vec::new()
        } else {
            agents::build_registry()
        };
        if agents::simulated::SimulatedSource::enabled() {
            sources.push(Box::new(agents::simulated::SimulatedSource::new()));
        }
        let descriptors: Vec<AgentDescriptor> = sources.iter().map(|s| s.descriptor()).collect();
        Self {
            sources,
            processes: Arc::new(ProcessTable::sample()),
            snapshot: Snapshot::new(Vec::new(), descriptors),
            last_hash: 0,
            dismissed: HashMap::new(),
        }
    }

    pub fn descriptors(&self) -> Vec<AgentDescriptor> {
        self.sources.iter().map(|s| s.descriptor()).collect()
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Re-scan every adapter. Returns `true` when the rendered content changed
    /// (caller then emits a UI event). Cheap when sources are unchanged.
    pub fn scan(&mut self, force: bool) -> bool {
        let now = crate::model::now_secs();
        if force || now_millis().saturating_sub(self.processes.sampled_at_ms) >= 2_000 {
            self.processes = Arc::new(ProcessTable::sample());
        }
        if force {
            for s in &mut self.sources {
                s.invalidate();
            }
        }
        let ctx = ScanCtx::new(now, self.processes.clone());

        let mut tasks: Vec<TaskRecord> = Vec::new();
        for source in &mut self.sources {
            tasks.extend(source.scan(&ctx));
        }

        // Dismissed records stay hidden until they show new activity.
        tasks.retain(|t| match self.dismissed.get(&t.id) {
            Some(at) => t.last_activity_secs > *at,
            None => true,
        });
        self.dismissed
            .retain(|id, at| tasks.iter().any(|t| &t.id == id && t.last_activity_secs > *at));

        // Defensive normalisation: adapters must set these, but a bad adapter
        // must never make the UI advertise a jump it cannot perform.
        for t in &mut tasks {
            if t.target_validity == TargetValidity::Invalid {
                t.capability.max_level = crate::model::JumpLevel::L0;
            }
            t.refresh_level();
        }

        agents::sort_tasks(&mut tasks);
        let snapshot = Snapshot::new(tasks, self.descriptors());
        let hash = snapshot.render_hash();
        self.snapshot = snapshot;
        if hash != self.last_hash {
            self.last_hash = hash;
            true
        } else {
            false
        }
    }

    pub fn dismiss(&mut self, id: &str) -> bool {
        if let Some(task) = self.snapshot.tasks.iter().find(|t| t.id == id) {
            self.dismissed.insert(id.to_string(), task.last_activity_secs);
            self.snapshot.tasks.retain(|t| t.id != id);
            self.last_hash = 0;
            true
        } else {
            false
        }
    }

    pub fn task(&self, id: &str) -> Option<&TaskRecord> {
        self.snapshot.tasks.iter().find(|t| t.id == id)
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}
