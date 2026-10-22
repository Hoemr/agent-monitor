//! Collects records from every adapter, keeps a single unified snapshot and
//! suppresses duplicate UI updates.
//!
//! The monitor also owns the two pieces of state that are not an adapter's
//! business:
//!   * the set of enabled agents — the registry is rebuilt when the user flips
//!     a switch in Settings,
//!   * dismissed task ids — persisted atomically to a caller-provided path so
//!     a restart does not resurrect rows the user already hid.

use crate::agents::{self, ProcessTable, ScanCtx, Source};
use crate::model::{now_millis, AgentDescriptor, Snapshot, TaskRecord, TargetValidity};
use crate::settings::EnabledAgents;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// On-disk shape of `dismissed.json`:
/// `{"version":1,"entries":{"<task id>": <last_activity_secs at dismissal>}}`.
const DISMISSED_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct DismissedFile {
    version: u32,
    #[serde(default)]
    entries: HashMap<String, u64>,
}

pub struct Monitor {
    sources: Vec<Box<dyn Source>>,
    processes: Arc<ProcessTable>,
    snapshot: Snapshot,
    last_hash: u64,
    /// id -> last_activity_secs at the moment the user dismissed it.
    dismissed: HashMap<String, u64>,
    /// Where dismissals are persisted. `None` = memory only (used by the
    /// headless `--dump-tasks` paths and by tests that do not care).
    dismissed_path: Option<PathBuf>,
    /// The switches the current registry was built from, so a settings save
    /// that did not touch them cannot needlessly rebuild every adapter.
    enabled_agents: EnabledAgents,
}

impl Monitor {
    /// Build the monitor for the given per-agent switches.
    ///
    /// Measurement aid: `AGENT_MONITOR_EMPTY=1` gives a truly empty snapshot so
    /// the "no tasks, idle" CPU budget can be measured without mocking data.
    /// The developer-only simulated adapter is appended last exactly as before
    /// (it is env-gated inside `SimulatedSource::enabled`).
    pub fn new(enabled_agents: &EnabledAgents) -> Self {
        Self::with_sources(build_sources(enabled_agents), enabled_agents.clone())
    }

    fn with_sources(sources: Vec<Box<dyn Source>>, enabled_agents: EnabledAgents) -> Self {
        let descriptors: Vec<AgentDescriptor> = sources.iter().map(|s| s.descriptor()).collect();
        Self {
            sources,
            processes: Arc::new(ProcessTable::sample()),
            snapshot: Snapshot::new(Vec::new(), descriptors),
            last_hash: 0,
            dismissed: HashMap::new(),
            dismissed_path: None,
            enabled_agents,
        }
    }

    /// Apply new per-agent switches.
    ///
    /// Returns `true` when the active registry actually changed — the caller
    /// should then `scan` immediately and emit the new snapshot. Returns
    /// `false` (no rebuild at all) when the switches are unchanged, e.g. a
    /// settings save that only touched the language.
    pub fn set_enabled_agents(&mut self, enabled_agents: &EnabledAgents) -> bool {
        if &self.enabled_agents == enabled_agents {
            return false;
        }
        self.enabled_agents = enabled_agents.clone();
        self.sources = build_sources(enabled_agents);
        // Force the next scan to emit even if the remaining rows were unchanged.
        self.last_hash = 0;
        true
    }

    pub fn descriptors(&self) -> Vec<AgentDescriptor> {
        self.sources.iter().map(|s| s.descriptor()).collect()
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Re-scan every adapter. Returns `true` when the rendered content changed
    /// (caller then emits a UI event). Cheap when sources are unchanged.
    ///
    /// `retention_secs` is the user-configured finished-work window from
    /// Settings and is injected into every adapter through [`ScanCtx`].
    pub fn scan(&mut self, force: bool, retention_secs: u64) -> bool {
        let now = crate::model::now_secs();
        if force || now_millis().saturating_sub(self.processes.sampled_at_ms) >= 2_000 {
            self.processes = Arc::new(ProcessTable::sample());
        }
        if force {
            for s in &mut self.sources {
                s.invalidate();
            }
        }
        let ctx = ScanCtx::new(now, self.processes.clone(), retention_secs);

        let mut tasks: Vec<TaskRecord> = Vec::new();
        for source in &mut self.sources {
            tasks.extend(source.scan(&ctx));
        }

        // Dismissed records stay hidden until they show new activity. The
        // reported activities are captured before filtering so the dismissal
        // bookkeeping below can tell "still hidden" from "overtaken".
        let reported: Vec<(String, u64)> = tasks
            .iter()
            .map(|t| (t.id.clone(), t.last_activity_secs))
            .collect();
        tasks.retain(|t| match self.dismissed.get(&t.id) {
            Some(at) => t.last_activity_secs > *at,
            None => true,
        });
        // Keep an entry only while the task is still hidden by it (or while the
        // adapter is not reporting the task at all). New activity consumes the
        // dismissal for good; this is memory-only because a temporary adapter
        // hiccup must not resurrect a row on disk (the stale file entry is
        // keyed to the old activity and stays harmless).
        self.dismissed.retain(|id, at| {
            match reported.iter().find(|(task_id, _)| task_id == id) {
                Some((_, activity)) => *activity <= *at,
                None => true,
            }
        });

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
            self.save_dismissed();
            true
        } else {
            false
        }
    }

    /// Forget one dismissal. The task becomes visible again on the next scan
    /// (the caller re-scans and emits). Returns whether an entry existed.
    pub fn undismiss(&mut self, id: &str) -> bool {
        let removed = self.dismissed.remove(id).is_some();
        if removed {
            self.last_hash = 0;
            self.save_dismissed();
        }
        removed
    }

    /// Forget every dismissal ("clear ignored tasks"). Returns how many were
    /// removed; the next scan makes every still-present task visible again.
    pub fn clear_dismissed(&mut self) -> usize {
        let cleared = self.dismissed.len();
        if cleared > 0 {
            self.dismissed.clear();
            self.last_hash = 0;
            self.save_dismissed();
        }
        cleared
    }

    pub fn dismissed_count(&self) -> usize {
        self.dismissed.len()
    }

    pub fn task(&self, id: &str) -> Option<&TaskRecord> {
        self.snapshot.tasks.iter().find(|t| t.id == id)
    }

    // ── Dismissal persistence ───────────────────────────────────

    /// Load persisted dismissals from `path` and use that path for every future
    /// write. Missing / corrupt / unknown-version files simply mean "nothing
    /// dismissed": startup is never blocked and the file is not deleted here.
    ///
    /// The loaded map replaces the in-memory one (call this once at startup).
    pub fn restore_dismissed(&mut self, path: PathBuf) {
        self.dismissed = Self::load_dismissed(&path);
        self.dismissed_path = Some(path);
    }

    fn load_dismissed(path: &Path) -> HashMap<String, u64> {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return HashMap::new();
        };
        match serde_json::from_str::<DismissedFile>(&raw) {
            Ok(file) if file.version == DISMISSED_VERSION => file.entries,
            Ok(file) => {
                eprintln!(
                    "dismissed: unsupported version {} in {}; starting empty",
                    file.version,
                    path.display()
                );
                HashMap::new()
            }
            Err(err) => {
                eprintln!(
                    "dismissed: ignoring unreadable {} ({err}); starting empty",
                    path.display()
                );
                HashMap::new()
            }
        }
    }

    /// Atomic (tmp + rename) best-effort write. The in-memory map stays the
    /// source of truth: a failed write is reported to the caller but never
    /// loses a dismissal or blocks the UI.
    fn save_dismissed(&self) -> bool {
        let Some(path) = &self.dismissed_path else {
            return true;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                eprintln!("dismissed: cannot create {}", parent.display());
                return false;
            }
        }
        let file = DismissedFile {
            version: DISMISSED_VERSION,
            entries: self.dismissed.clone(),
        };
        let Ok(json) = serde_json::to_string_pretty(&file) else {
            return false;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_err() {
            eprintln!("dismissed: cannot write {}", tmp.display());
            return false;
        }
        if std::fs::rename(&tmp, path).is_err() {
            eprintln!("dismissed: cannot replace {}", path.display());
            return false;
        }
        true
    }

    /// Test-only constructor: inject fake sources and a pre-sampled process
    /// table so unit tests never spawn `ps` on every scan.
    #[cfg(test)]
    fn for_tests(sources: Vec<Box<dyn Source>>) -> Self {
        let descriptors: Vec<AgentDescriptor> = sources.iter().map(|s| s.descriptor()).collect();
        Self {
            sources,
            processes: Arc::new(ProcessTable {
                entries: Vec::new(),
                sampled_at_ms: now_millis(),
            }),
            snapshot: Snapshot::new(Vec::new(), descriptors),
            last_hash: 0,
            dismissed: HashMap::new(),
            dismissed_path: None,
            enabled_agents: EnabledAgents::default(),
        }
    }
}

/// Build the adapter registry for `enabled`, honouring the empty-snapshot
/// measurement aid and the env-gated simulated adapter.
fn build_sources(enabled: &EnabledAgents) -> Vec<Box<dyn Source>> {
    let mut sources = if std::env::var("AGENT_MONITOR_EMPTY").as_deref() == Ok("1") {
        Vec::new()
    } else {
        agents::build_registry(enabled)
    };
    if agents::simulated::SimulatedSource::enabled() {
        sources.push(Box::new(agents::simulated::SimulatedSource::new()));
    }
    sources
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new(&EnabledAgents::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    // ── Test doubles ────────────────────────────────────────────

    #[derive(Clone, Default)]
    struct Probe {
        retention_secs: Arc<AtomicU64>,
        scans: Arc<AtomicU64>,
    }

    /// Adapter whose rows live behind a shared `Mutex` so a test can advance a
    /// task's activity between scans; it also records the injected retention.
    struct FakeSource {
        key: String,
        records: Arc<Mutex<Vec<TaskRecord>>>,
        probe: Probe,
    }

    impl Source for FakeSource {
        fn descriptor(&self) -> AgentDescriptor {
            AgentDescriptor {
                key: self.key.clone(),
                name: self.key.clone(),
                source: "unit-test fixture".into(),
                origin: DataOrigin::Synthetic,
                max_level: JumpLevel::L1,
                notes: "test".into(),
            }
        }

        fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
            self.probe
                .retention_secs
                .store(ctx.retention_secs, Ordering::SeqCst);
            self.probe.scans.fetch_add(1, Ordering::SeqCst);
            self.records.lock().unwrap().clone()
        }
    }

    fn task(id: &str, status: TaskStatus, activity_secs: u64) -> TaskRecord {
        let mut t = TaskRecord {
            id: id.into(),
            agent_key: "fake".into(),
            agent_name: "Fake".into(),
            host: "desktop".into(),
            session_id: Some(id.into()),
            task_id: None,
            title: id.into(),
            status,
            status_reason: "fixture".into(),
            last_activity_secs: activity_secs,
            collected_at_secs: activity_secs,
            summary: None,
            origin: DataOrigin::Synthetic,
            target: JumpTarget::default(),
            target_validity: TargetValidity::Valid,
            invalid_reason: None,
            capability: JumpCapability::default(),
            level: JumpLevel::L1,
            pinned: false,
        };
        t.refresh_level();
        t
    }

    fn monitor_with(
        records: Arc<Mutex<Vec<TaskRecord>>>,
        probe: &Probe,
    ) -> Monitor {
        Monitor::for_tests(vec![Box::new(FakeSource {
            key: "fake".into(),
            records,
            probe: probe.clone(),
        })])
    }

    fn temp_path(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "am-monitor-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.join("dismissed.json"), dir)
    }

    fn visible_ids(monitor: &Monitor) -> Vec<String> {
        let mut ids: Vec<String> = monitor.snapshot().tasks.iter().map(|t| t.id.clone()).collect();
        ids.sort();
        ids
    }

    fn read_entries(path: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(path).expect("dismissed file must exist");
        serde_json::from_str::<serde_json::Value>(&raw).expect("dismissed file must be JSON")
    }

    // ── Registry / injection ────────────────────────────────────

    #[test]
    fn scan_carries_the_injected_retention_into_every_adapter() {
        let probe = Probe::default();
        let mut monitor = monitor_with(Arc::new(Mutex::new(Vec::new())), &probe);
        monitor.scan(false, 12_345);
        assert_eq!(probe.retention_secs.load(Ordering::SeqCst), 12_345);
        monitor.scan(false, 60);
        assert_eq!(probe.retention_secs.load(Ordering::SeqCst), 60);
    }

    #[test]
    fn set_enabled_agents_rebuilds_and_reports_only_real_changes() {
        let probe = Probe::default();
        let records = Arc::new(Mutex::new(vec![task("fake:a", TaskStatus::Running, 100)]));
        let mut monitor = monitor_with(records, &probe);

        // Same switches as the registry was built from: no rebuild, no work.
        assert!(!monitor.set_enabled_agents(&EnabledAgents::default()));
        assert_eq!(monitor.descriptors().len(), 1);
        assert_eq!(monitor.descriptors()[0].key, "fake");

        // A real change rebuilds the production registry minus the disabled
        // agents (claude-code + zcode remain, in their stable order).
        let mut enabled = EnabledAgents::default();
        enabled.codex = false;
        enabled.dsh = false;
        assert!(monitor.set_enabled_agents(&enabled));
        let keys: Vec<String> = monitor
            .descriptors()
            .iter()
            .map(|d| d.key.clone())
            .collect();
        assert_eq!(keys, vec!["claude-code", "zcode"]);

        // Re-applying the same value is a no-op.
        assert!(!monitor.set_enabled_agents(&enabled));
    }

    #[test]
    fn descriptors_reflect_the_enabled_agents_at_construction() {
        let enabled = EnabledAgents {
            codex: false,
            zcode: false,
            ..Default::default()
        };
        // Guard against the measurement override being set in the test env.
        if std::env::var("AGENT_MONITOR_EMPTY").as_deref() == Ok("1") {
            return;
        }
        let monitor = Monitor::new(&enabled);
        let keys: Vec<String> = monitor
            .descriptors()
            .iter()
            .map(|d| d.key.clone())
            .collect();
        assert_eq!(keys, vec!["claude-code", "dsh"]);
    }

    // ── Dismissal persistence ───────────────────────────────────

    #[test]
    fn dismiss_persists_the_documented_format_atomically_and_restores_it() {
        let (path, dir) = temp_path("persist");
        let probe = Probe::default();
        let records = Arc::new(Mutex::new(vec![
            task("fake:a", TaskStatus::Running, 100),
            task("fake:b", TaskStatus::Completed, 50),
        ]));
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.restore_dismissed(path.clone());
        monitor.scan(false, 3_600);
        assert!(monitor.dismiss("fake:a"));

        let json = read_entries(&path);
        assert_eq!(json["version"], 1);
        assert_eq!(json["entries"]["fake:a"], 100);
        // Atomic write: the temp file must not be left behind.
        assert!(!path.with_extension("json.tmp").exists());
        assert_eq!(monitor.dismissed_count(), 1);
        assert!(!visible_ids(&monitor).contains(&"fake:a".to_string()));

        // A fresh monitor restoring the same file hides the row while its
        // activity is unchanged.
        let mut restored = monitor_with(records, &probe);
        restored.restore_dismissed(path.clone());
        restored.scan(false, 3_600);
        assert_eq!(visible_ids(&restored), vec!["fake:b".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn new_activity_after_a_dismissal_brings_the_task_back() {
        let (path, dir) = temp_path("activity");
        let probe = Probe::default();
        let records = Arc::new(Mutex::new(vec![task("fake:a", TaskStatus::Running, 100)]));
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.restore_dismissed(path);
        monitor.scan(false, 3_600);
        assert!(monitor.dismiss("fake:a"));
        assert!(visible_ids(&monitor).is_empty());

        // The adapter reports new activity for the same id: the dismissal is
        // consumed and the row is visible again.
        records.lock().unwrap()[0].last_activity_secs = 200;
        monitor.scan(false, 3_600);
        assert_eq!(visible_ids(&monitor), vec!["fake:a".to_string()]);
        assert_eq!(monitor.dismissed_count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undismiss_and_clear_dismissed_rewrite_the_persisted_file() {
        let (path, dir) = temp_path("clear");
        let probe = Probe::default();
        let records = Arc::new(Mutex::new(vec![
            task("fake:a", TaskStatus::Running, 100),
            task("fake:b", TaskStatus::Completed, 100),
        ]));
        let mut monitor = monitor_with(records, &probe);
        monitor.restore_dismissed(path.clone());
        monitor.scan(false, 3_600);
        assert!(monitor.dismiss("fake:a"));
        assert!(monitor.dismiss("fake:b"));
        assert_eq!(monitor.dismissed_count(), 2);

        assert!(monitor.undismiss("fake:a"));
        assert!(!monitor.undismiss("fake:a"));
        assert_eq!(monitor.dismissed_count(), 1);
        let json = read_entries(&path);
        assert!(json["entries"].get("fake:a").is_none());
        assert_eq!(json["entries"]["fake:b"], 100);

        // One dismissal remains: clearing reports exactly that count and makes
        // both rows visible on the next scan.
        assert_eq!(monitor.clear_dismissed(), 1);
        assert_eq!(monitor.dismissed_count(), 0);
        let json = read_entries(&path);
        assert_eq!(json["entries"].as_object().unwrap().len(), 0);
        monitor.scan(false, 3_600);
        assert_eq!(
            visible_ids(&monitor),
            vec!["fake:a".to_string(), "fake:b".to_string()]
        );
        assert_eq!(monitor.clear_dismissed(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_or_unknown_version_files_are_ignored_without_blocking_startup() {
        let (corrupt, dir) = temp_path("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&corrupt, "{ not json").unwrap();

        let probe = Probe::default();
        let records = Arc::new(Mutex::new(vec![task("fake:a", TaskStatus::Running, 100)]));
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.restore_dismissed(corrupt.clone());
        monitor.scan(false, 3_600);
        assert_eq!(visible_ids(&monitor), vec!["fake:a".to_string()]);
        // A later dismissal overwrites the broken file with the valid format.
        assert!(monitor.dismiss("fake:a"));
        assert_eq!(read_entries(&corrupt)["version"], 1);

        let mut newer = monitor_with(records, &probe);
        let (unknown, dir2) = temp_path("unknown-version");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            &unknown,
            r#"{"version":2,"entries":{"fake:a":100}}"#,
        )
        .unwrap();
        newer.restore_dismissed(unknown);
        newer.scan(false, 3_600);
        assert_eq!(visible_ids(&newer), vec!["fake:a".to_string()]);
        assert_eq!(newer.dismissed_count(), 0);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(dir2);
    }

    #[test]
    fn dismissing_an_unknown_task_is_a_noop_and_writes_nothing() {
        let (path, dir) = temp_path("noop");
        let probe = Probe::default();
        let mut monitor = monitor_with(Arc::new(Mutex::new(Vec::new())), &probe);
        monitor.restore_dismissed(path.clone());
        monitor.scan(false, 3_600);
        assert!(!monitor.dismiss("does-not-exist"));
        assert_eq!(monitor.dismissed_count(), 0);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dismissals_are_memory_only_without_a_restored_path() {
        let probe = Probe::default();
        let records = Arc::new(Mutex::new(vec![task("fake:a", TaskStatus::Running, 100)]));
        let mut monitor = monitor_with(records, &probe);
        monitor.scan(false, 3_600);
        assert!(monitor.dismiss("fake:a"));
        assert_eq!(monitor.dismissed_count(), 1);
        assert!(monitor.undismiss("fake:a"));
        assert_eq!(monitor.clear_dismissed(), 0);
    }
}
