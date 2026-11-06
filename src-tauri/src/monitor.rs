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
use crate::model::{now_millis, AgentDescriptor, Snapshot, TaskRecord, TaskStatus, TargetValidity};
use crate::settings::EnabledAgents;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// At most this many new questions are reported per batch (`docs/ATTENTION.md`);
/// the notification text collapses a batch into one message anyway.
pub const MAX_ALERTS: usize = 8;

/// Minimum interval between two alerts for the **same** task. Some adapters can
/// advance a waiting task's activity while it is still blocked (the synthetic
/// fixture ticks every scan); without this guard the same question would notify
/// on every scan. A task that newly becomes pending always alerts immediately.
pub const ALERT_COOLDOWN_SECS: u64 = 60;

/// One newly observed pending question. Serialised into the `attention-pending`
/// Tauri event; the field names are part of the frontend contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAlert {
    pub task_id: String,
    pub agent_key: String,
    pub agent_name: String,
    pub title: String,
    /// `waiting_input` / `waiting_approval` (snake_case, same as the snapshot).
    pub status: String,
    pub status_reason: String,
}

/// Fingerprint of a visible pending question: a question is "new" when its id is
/// absent from the previous map or this tuple changed.
type QuestionFingerprint = (TaskStatus, u64);

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
    pins: HashSet<String>,
    pins_path: Option<PathBuf>,
    custom_names: std::collections::BTreeMap<String, String>,
    custom_names_path: Option<PathBuf>,
    /// The switches the current registry was built from, so a settings save
    /// that did not touch them cannot needlessly rebuild every adapter.
    enabled_agents: EnabledAgents,
    /// Fingerprints of every *currently visible* pending question, keyed by task
    /// id. Replaced on every scan; only the delta produces alerts.
    question_fingerprints: HashMap<String, QuestionFingerprint>,
    /// id -> unix secs of the last alert, used by [`ALERT_COOLDOWN_SECS`].
    /// Entries are dropped together with the fingerprint (task no longer
    /// visible as a pending question).
    question_alert_at: HashMap<String, u64>,
    /// `false` until the first scan has seeded the fingerprint map: existing old
    /// questions at launch must not produce a burst of notifications.
    question_seeded: bool,
    /// New questions found by the latest scan, drained by the caller (lib.rs)
    /// after it has emitted the snapshot. Replaced on every scan.
    pending_questions: Vec<PendingAlert>,
}

impl Monitor {
    /// Build the monitor for the given per-agent switches.
    ///
    /// Measurement aid: `AGENT_MONITOR_EMPTY=1` gives a truly empty snapshot so
    /// the "no tasks, idle" CPU budget can be measured without mocking data.
    /// The developer-only simulated adapter is appended last exactly as before
    /// (it is env-gated inside `SimulatedSource::enabled`).
    pub fn new(enabled_agents: &EnabledAgents) -> Self {
        let mut monitor = Self::with_sources(build_sources(enabled_agents), enabled_agents.clone());
        if let Err(error) = monitor.load_pins(crate::settings::data_dir().join("pins.json")) {
            eprintln!("pins: {error}");
        }
        if let Err(error) = monitor.load_custom_names(crate::settings::data_dir().join("session-names.json")) {
            eprintln!("session names: {error}");
        }
        monitor
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
            pins: HashSet::new(),
            pins_path: None,
            custom_names: Default::default(),
            custom_names_path: None,
            enabled_agents,
            question_fingerprints: HashMap::new(),
            question_alert_at: HashMap::new(),
            question_seeded: false,
            pending_questions: Vec::new(),
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
        // Re-seed the question detector: a disabled agent's rows are not
        // collected (so they can never alert), and re-enabling it must not
        // replay every old question as if it had just appeared.
        self.question_fingerprints.clear();
        self.question_alert_at.clear();
        self.question_seeded = false;
        self.pending_questions.clear();
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
        let mut ctx = ScanCtx::new(now, self.processes.clone(), retention_secs);
        ctx.pinned_ids = Arc::new(self.pins.clone());

        let mut tasks: Vec<TaskRecord> = Vec::new();
        for source in &mut self.sources {
            for mut task in source.scan(&ctx) {
                task.enforce_completion();
                task.pinned = self.pins.contains(&task.id);
                tasks.push(task);
            }
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
        let mut snapshot = Snapshot::new(tasks, self.descriptors());
        snapshot.custom_names = self.custom_names.clone();
        // Detection runs on the *visible* rows only: dismissed tasks were
        // filtered above and disabled agents were never collected.
        self.detect_new_questions(&snapshot.tasks, now);
        let hash = snapshot.render_hash();
        self.snapshot = snapshot;
        if hash != self.last_hash {
            self.last_hash = hash;
            true
        } else {
            false
        }
    }

    /// Compare the visible pending questions with the previous scan and record
    /// the new/changed ones for the caller to surface.
    ///
    /// Rules (docs/ATTENTION.md):
    /// * the first scan only seeds the fingerprint map;
    /// * a repeated identical waiting row does not alert again;
    /// * a changed `(status, last_activity_secs)` — e.g. running → waiting, or
    ///   a new question in the same session — alerts again, but a task that was
    ///   already pending re-alerts at most once per [`ALERT_COOLDOWN_SECS`];
    /// * non-question statuses are ignored entirely.
    fn detect_new_questions(&mut self, tasks: &[TaskRecord], now_secs: u64) {
        let mut current: HashMap<String, QuestionFingerprint> = HashMap::new();
        let mut new_questions: Vec<PendingAlert> = Vec::new();
        let mut alerted: Vec<(String, u64)> = Vec::new();
        for task in tasks {
            if !task.status.is_pending_question() {
                continue;
            }
            let fingerprint = (task.status, task.last_activity_secs);
            let previous = self.question_fingerprints.get(&task.id);
            let is_new = previous != Some(&fingerprint);
            // Still advance the fingerprint even when the alert is rate-limited:
            // the suppressed tick must not surface as an alert later.
            current.insert(task.id.clone(), fingerprint);
            if !(is_new && self.question_seeded) {
                continue;
            }
            let already_pending = previous.is_some();
            let last_alert_at = self.question_alert_at.get(&task.id).copied();
            if !question_alert_allowed(already_pending, last_alert_at, now_secs) {
                continue;
            }
            new_questions.push(PendingAlert {
                task_id: task.id.clone(),
                agent_key: task.agent_key.clone(),
                agent_name: task.agent_name.clone(),
                title: self.custom_names.get(&task.id).unwrap_or(&task.title).clone(),
                status: task.status.key().to_string(),
                status_reason: task.status_reason.clone(),
            });
            alerted.push((task.id.clone(), now_secs));
        }
        self.question_seeded = true;
        // Cooldown state only matters for questions that are still visible.
        self.question_alert_at.retain(|id, _| current.contains_key(id));
        for (id, at) in alerted {
            self.question_alert_at.insert(id, at);
        }
        self.question_fingerprints = current;
        // `tasks` is sorted newest-activity first, so the cap keeps the newest
        // questions (the ones most likely to still block the user).
        new_questions.truncate(MAX_ALERTS);
        self.pending_questions = new_questions;
    }

    /// Test-only: age the recorded alert so the cooldown-expiry branch can be
    /// exercised without sleeping a real minute.
    #[cfg(test)]
    fn backdate_question_alert(&mut self, id: &str, age_secs: u64) {
        let at = crate::model::now_secs().saturating_sub(age_secs);
        self.question_alert_at.insert(id.to_string(), at);
    }

    /// Drain the new pending questions discovered by the latest [`Self::scan`].
    ///
    /// The caller (lib.rs) emits the `attention-pending` event and the macOS
    /// notification from this batch. The list is replaced by every scan, so
    /// call this right after a scan; an empty result means "nothing new".
    pub fn take_new_questions(&mut self) -> Vec<PendingAlert> {
        std::mem::take(&mut self.pending_questions)
    }

    pub fn load_custom_names(&mut self, path: PathBuf) -> Result<(), String> {
        self.custom_names_path = Some(path.clone());
        self.custom_names = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("invalid session-names.json: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(e) => return Err(format!("cannot read session-names.json: {e}")),
        };
        Ok(())
    }

    pub fn set_custom_name(&mut self, id: &str, name: &str) -> Result<(), String> {
        if !self.snapshot.tasks.iter().any(|task| task.id == id) {
            return Err("task_gone".into());
        }
        let name = name.trim();
        if name.chars().count() > 120 || name.chars().any(char::is_control) {
            return Err("Use at most 120 characters on one line.".into());
        }
        let mut next = self.custom_names.clone();
        if name.is_empty() { next.remove(id); } else { next.insert(id.to_string(), name.to_string()); }
        if let Some(path) = &self.custom_names_path {
            // Refuse to overwrite an unreadable existing file after a load error.
            if path.exists() {
                let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                serde_json::from_str::<std::collections::BTreeMap<String, String>>(&text).map_err(|e| e.to_string())?;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let temp = path.with_extension("json.tmp");
            std::fs::write(&temp, serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            std::fs::rename(temp, path).map_err(|e| e.to_string())?;
        }
        self.custom_names = next;
        self.snapshot.custom_names = self.custom_names.clone();
        self.last_hash = 0;
        Ok(())
    }

    pub fn load_pins(&mut self, path: PathBuf) -> Result<(), String> {
        self.pins_path = Some(path.clone());
        self.pins = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("invalid pins.json: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
            Err(e) => return Err(format!("cannot read pins.json: {e}")),
        };
        Ok(())
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> Result<(), String> {
        if !self.snapshot.tasks.iter().any(|task| task.id == id) {
            return Err("task_gone".into());
        }
        let mut next = self.pins.clone();
        if pinned { next.insert(id.to_string()); } else { next.remove(id); }
        if let Some(path) = &self.pins_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut ids: Vec<_> = next.iter().collect();
            ids.sort();
            let temp = path.with_extension("json.tmp");
            std::fs::write(&temp, serde_json::to_vec(&ids).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            std::fs::rename(temp, path).map_err(|e| e.to_string())?;
        }
        self.pins = next;
        for task in &mut self.snapshot.tasks { task.pinned = self.pins.contains(&task.id); }
        agents::sort_tasks(&mut self.snapshot.tasks);
        self.last_hash = 0;
        Ok(())
    }

    pub fn dismiss(&mut self, id: &str) -> bool {
        if let Some(task) = self.snapshot.tasks.iter().find(|t| t.id == id) {
            self.dismissed.insert(id.to_string(), task.last_activity_secs);
            self.snapshot.tasks.retain(|t| t.id != id);
            // The badge/header counters are derived from the visible rows.
            self.snapshot.recount();
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
            pins: HashSet::new(),
            pins_path: None,
            custom_names: Default::default(),
            custom_names_path: None,
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
            question_fingerprints: HashMap::new(),
            question_alert_at: HashMap::new(),
            question_seeded: false,
            pending_questions: Vec::new(),
        }
    }
}

/// Re-alert rule for an already-pending question: at most once per
/// [`ALERT_COOLDOWN_SECS`]. A task that was not pending before (first
/// appearance, running → waiting) always alerts immediately.
fn question_alert_allowed(
    already_pending: bool,
    last_alert_at: Option<u64>,
    now_secs: u64,
) -> bool {
    if !already_pending {
        return true;
    }
    match last_alert_at {
        // Boundary is inclusive: at exactly ALERT_COOLDOWN_SECS it may alert.
        Some(at) => now_secs.saturating_sub(at) >= ALERT_COOLDOWN_SECS,
        None => true,
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
            completion_id: Some("100".into()),
            completion: (status == TaskStatus::Completed).then_some(CompletionEvidence::TaskEnd),
            status_reason: "fixture".into(),
            last_activity_secs: activity_secs,
            collected_at_secs: activity_secs,
            summary: None,
            model: None,
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
    fn custom_names_survive_scans_restart_and_reset_without_changing_source_titles() {
        let (path, dir) = temp_path("names");
        let records = Arc::new(Mutex::new(vec![task("fake:one", TaskStatus::Completed, 100), task("fake:two", TaskStatus::Running, 200)]));
        let probe = Probe::default();
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.load_custom_names(path.clone()).unwrap();
        monitor.scan(false, u64::MAX);
        let hash = monitor.snapshot().render_hash();
        monitor.set_custom_name("fake:one", "  我的研究  ").unwrap();
        assert_ne!(hash, monitor.snapshot().render_hash());
        monitor.scan(false, u64::MAX);
        assert_eq!(monitor.snapshot().custom_names["fake:one"], "我的研究");
        assert_eq!(monitor.task("fake:one").unwrap().title, "fake:one");
        assert!(!monitor.snapshot().custom_names.contains_key("fake:two"));
        let mut restarted = monitor_with(records, &probe);
        restarted.load_custom_names(path.clone()).unwrap();
        restarted.scan(false, u64::MAX);
        assert_eq!(restarted.snapshot().custom_names["fake:one"], "我的研究");
        assert!(restarted.set_custom_name("fake:one", &"字".repeat(121)).is_err());
        assert!(restarted.set_custom_name("missing", "name").is_err());
        restarted.set_custom_name("fake:one", " ").unwrap();
        assert!(restarted.snapshot().custom_names.is_empty());
        assert_eq!(read_entries(&path), serde_json::json!({}));
        // A failed atomic save must not update the visible name.
        restarted.custom_names_path = Some(dir.join("missing-parent").join("names.json"));
        std::fs::write(dir.join("missing-parent"), "file, not directory").unwrap();
        assert!(restarted.set_custom_name("fake:one", "unsaved").is_err());
        assert!(restarted.snapshot().custom_names.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pins_survive_restart_and_scans_and_unpin_restores_activity_order() {
        let (path, dir) = temp_path("pins");
        let pins = path.with_file_name("pins.json");
        let records = Arc::new(Mutex::new(vec![
            task("fake:old", TaskStatus::Completed, 1),
            task("fake:new", TaskStatus::Running, 100),
        ]));
        let probe = Probe::default();
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.load_pins(pins.clone()).unwrap();
        monitor.scan(false, u64::MAX);
        monitor.set_pinned("fake:old", true).unwrap();
        monitor.scan(false, u64::MAX);
        assert_eq!(monitor.snapshot().tasks[0].id, "fake:old");
        assert!(monitor.snapshot().tasks[0].pinned);
        let mut restarted = monitor_with(records, &probe);
        restarted.load_pins(pins).unwrap();
        restarted.scan(false, u64::MAX);
        assert_eq!(restarted.snapshot().tasks[0].id, "fake:old");
        restarted.set_pinned("fake:old", false).unwrap();
        restarted.scan(false, u64::MAX);
        assert_eq!(restarted.snapshot().tasks[0].id, "fake:new");
        assert!(restarted.set_pinned("missing", true).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_adapter_must_provide_final_completion_evidence() {
        let mut unsupported = task("future:unsupported", TaskStatus::Completed, 100);
        unsupported.completion = None;
        let confirmed = task("future:confirmed", TaskStatus::Completed, 100);
        let records = Arc::new(Mutex::new(vec![unsupported, confirmed]));
        let mut monitor = monitor_with(records, &Probe::default());
        monitor.scan(false, u64::MAX);
        let find = |id| monitor.snapshot().tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(find("future:unsupported").status, TaskStatus::Unknown);
        assert!(find("future:unsupported").status_reason.contains("unconfirmed"));
        assert_eq!(find("future:confirmed").status, TaskStatus::Completed);
    }

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
        assert_eq!(keys, vec!["claude-code", "zcode", "antigravity"]);

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
        assert_eq!(keys, vec!["claude-code", "dsh", "antigravity"]);
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

    // ── Pending-question detector ───────────────────────────────

    fn records_with(tasks: Vec<TaskRecord>) -> Arc<Mutex<Vec<TaskRecord>>> {
        Arc::new(Mutex::new(tasks))
    }

    fn set_status(records: &Arc<Mutex<Vec<TaskRecord>>>, id: &str, status: TaskStatus, activity: u64) {
        let mut guard = records.lock().unwrap();
        let t = guard
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap_or_else(|| panic!("fixture task {id} missing"));
        t.status = status;
        t.last_activity_secs = activity;
    }

    #[test]
    fn first_scan_only_seeds_question_fingerprints() {
        let probe = Probe::default();
        let records = records_with(vec![
            task("fake:input", TaskStatus::WaitingInput, 100),
            task("fake:approval", TaskStatus::WaitingApproval, 90),
        ]);
        let mut monitor = monitor_with(records, &probe);
        monitor.scan(false, 3_600);
        assert!(
            monitor.take_new_questions().is_empty(),
            "questions that already existed at launch must only seed the baseline"
        );
        assert_eq!(monitor.snapshot().question_count, 2);
        // The identical next scan stays silent too.
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
    }

    #[test]
    fn new_waiting_task_alerts_with_the_documented_fields() {
        let probe = Probe::default();
        let records = records_with(vec![task("fake:run", TaskStatus::Running, 100)]);
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());

        records
            .lock()
            .unwrap()
            .push(task("fake:q", TaskStatus::WaitingInput, 200));
        monitor.scan(false, 3_600);
        let alerts = monitor.take_new_questions();
        assert_eq!(alerts.len(), 1);
        let alert = &alerts[0];
        assert_eq!(alert.task_id, "fake:q");
        assert_eq!(alert.agent_key, "fake");
        assert_eq!(alert.agent_name, "Fake");
        assert_eq!(alert.title, "fake:q");
        assert_eq!(alert.status, "waiting_input");
        assert_eq!(alert.status_reason, "fixture");
        assert_eq!(monitor.snapshot().question_count, 1);
        // Drained: the same batch is never delivered twice.
        assert!(monitor.take_new_questions().is_empty());
    }

    #[test]
    fn pending_alert_serialises_the_frontend_contract() {
        let alert = PendingAlert {
            task_id: "dsh:session-1".into(),
            agent_key: "dsh".into(),
            agent_name: "DSH".into(),
            title: "多窗口询问提醒与跳转".into(),
            status: "waiting_approval".into(),
            status_reason: "1 active user question(s)".into(),
        };
        let json = serde_json::to_value(&alert).unwrap();
        for key in [
            "task_id",
            "agent_key",
            "agent_name",
            "title",
            "status",
            "status_reason",
        ] {
            assert!(json.get(key).is_some(), "missing key {key} in {json}");
        }
        assert_eq!(json["status"], "waiting_approval");
        let roundtrip: PendingAlert = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip, alert);
    }

    #[test]
    fn repeated_identical_waiting_scan_does_not_realert() {
        let probe = Probe::default();
        let records = records_with(vec![task("fake:q", TaskStatus::WaitingInput, 100)]);
        let mut monitor = monitor_with(records, &probe);
        monitor.scan(false, 3_600); // seeds
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
        // An explicit user refresh must not replay the notification either.
        monitor.scan(true, 3_600);
        assert!(monitor.take_new_questions().is_empty());
    }

    #[test]
    fn waiting_activity_tick_within_cooldown_is_silent() {
        let probe = Probe::default();
        let records = records_with(Vec::new());
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds empty baseline

        // First appearance alerts immediately and starts the cooldown.
        records
            .lock()
            .unwrap()
            .push(task("fake:q", TaskStatus::WaitingInput, 100));
        monitor.scan(false, 3_600);
        assert_eq!(monitor.take_new_questions().len(), 1);

        // Adapters may keep ticking activity while the agent is still blocked:
        // the same question must not notify again inside the cooldown.
        set_status(&records, "fake:q", TaskStatus::WaitingInput, 101);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
        // A status change while still pending is the same blocking question too.
        set_status(&records, "fake:q", TaskStatus::WaitingApproval, 102);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
    }

    #[test]
    fn waiting_activity_change_after_cooldown_realerts() {
        let probe = Probe::default();
        let records = records_with(Vec::new());
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds empty baseline

        records
            .lock()
            .unwrap()
            .push(task("fake:q", TaskStatus::WaitingInput, 100));
        monitor.scan(false, 3_600);
        assert_eq!(monitor.take_new_questions().len(), 1);

        // Age the recorded alert past the cooldown: a further activity change is
        // a genuinely new blocking state and may alert again.
        monitor.backdate_question_alert("fake:q", ALERT_COOLDOWN_SECS + 1);
        set_status(&records, "fake:q", TaskStatus::WaitingInput, 200);
        monitor.scan(false, 3_600);
        assert_eq!(monitor.take_new_questions().len(), 1);

        // The new alert restarts the cooldown.
        set_status(&records, "fake:q", TaskStatus::WaitingInput, 201);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
    }

    #[test]
    fn cooldown_rule_boundary_and_new_questions() {
        // Not previously pending → always allowed, regardless of any timer.
        assert!(question_alert_allowed(false, Some(999), 1_000));
        assert!(question_alert_allowed(false, None, 1_000));
        // Already pending: suppressed strictly inside the window...
        assert!(!question_alert_allowed(true, Some(1_000), 1_000));
        assert!(!question_alert_allowed(
            true,
            Some(1_000),
            1_000 + ALERT_COOLDOWN_SECS - 1
        ));
        // ...and allowed at/after the boundary.
        assert!(question_alert_allowed(
            true,
            Some(1_000),
            1_000 + ALERT_COOLDOWN_SECS
        ));
        // A seeded question that was never alerted is not rate-limited.
        assert!(question_alert_allowed(true, None, 1_000));
    }

    #[test]
    fn running_then_waiting_alerts_immediately_despite_cooldown() {
        let probe = Probe::default();
        let records = records_with(vec![task("fake:q", TaskStatus::WaitingInput, 100)]);
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds

        set_status(&records, "fake:q", TaskStatus::Running, 150);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty(), "running is not a question");

        // running → waiting is a new question: no cooldown applies.
        set_status(&records, "fake:q", TaskStatus::WaitingInput, 150);
        monitor.scan(false, 3_600);
        assert_eq!(monitor.take_new_questions().len(), 1);

        // Even immediately afterwards, a new run → new question alerts again.
        set_status(&records, "fake:q", TaskStatus::Running, 160);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
        set_status(&records, "fake:q", TaskStatus::WaitingInput, 160);
        monitor.scan(false, 3_600);
        assert_eq!(monitor.take_new_questions().len(), 1);
    }

    #[test]
    fn non_question_statuses_never_alert() {
        let probe = Probe::default();
        let records = records_with(Vec::new());
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds empty baseline

        records.lock().unwrap().extend(vec![
            task("fake:failed", TaskStatus::Failed, 100),
            task("fake:completed", TaskStatus::Completed, 100),
            task("fake:running", TaskStatus::Running, 100),
            task("fake:idle", TaskStatus::Idle, 100),
            task("fake:unknown", TaskStatus::Unknown, 100),
        ]);
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
        assert_eq!(monitor.snapshot().question_count, 0);
    }

    #[test]
    fn dismissed_waiting_tasks_never_alert() {
        let probe = Probe::default();
        let records = records_with(vec![task("fake:q", TaskStatus::WaitingInput, 100)]);
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds
        assert!(monitor.dismiss("fake:q"));
        // Dismissal is reflected in the derived counter immediately.
        assert_eq!(monitor.snapshot().question_count, 0);

        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());

        // A different, genuinely new question still alerts.
        records
            .lock()
            .unwrap()
            .push(task("fake:new", TaskStatus::WaitingApproval, 300));
        monitor.scan(false, 3_600);
        let alerts = monitor.take_new_questions();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].task_id, "fake:new");
        assert_eq!(alerts[0].status, "waiting_approval");
    }

    #[test]
    fn new_questions_are_capped_per_batch() {
        let probe = Probe::default();
        let records = records_with(Vec::new());
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds empty baseline

        {
            let mut guard = records.lock().unwrap();
            for i in 0..(MAX_ALERTS + 4) {
                guard.push(task(
                    &format!("fake:q{i}"),
                    TaskStatus::WaitingInput,
                    100 + i as u64,
                ));
            }
        }
        monitor.scan(false, 3_600);
        let alerts = monitor.take_new_questions();
        assert_eq!(alerts.len(), MAX_ALERTS);
        // Newest activity first: the cap keeps the most recent questions.
        assert_eq!(alerts[0].task_id, format!("fake:q{}", MAX_ALERTS + 3));
        assert_eq!(alerts[MAX_ALERTS - 1].task_id, "fake:q4");
        // All visible questions still count, even beyond the batch cap.
        assert_eq!(monitor.snapshot().question_count, MAX_ALERTS + 4);
    }

    #[test]
    fn questions_not_collected_by_any_adapter_never_alert() {
        let probe = Probe::default();
        let records = records_with(vec![task("fake:q", TaskStatus::WaitingInput, 100)]);
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds
        records.lock().unwrap().clear();
        monitor.scan(false, 3_600);
        assert!(monitor.take_new_questions().is_empty());
        assert_eq!(monitor.snapshot().question_count, 0);
    }

    #[test]
    fn registry_change_reseeds_the_question_detector() {
        let probe = Probe::default();
        let records = records_with(vec![task("fake:q", TaskStatus::WaitingInput, 100)]);
        let mut monitor = monitor_with(records.clone(), &probe);
        monitor.scan(false, 3_600); // seeds
        records
            .lock()
            .unwrap()
            .push(task("fake:q2", TaskStatus::WaitingInput, 200));
        monitor.scan(false, 3_600);
        assert_eq!(monitor.take_new_questions().len(), 1);

        // Toggling agents rebuilds the registry; the new registry must start from
        // a fresh baseline instead of replaying every waiting row.
        let mut enabled = EnabledAgents::default();
        enabled.codex = false;
        assert!(monitor.set_enabled_agents(&enabled));
        assert!(monitor.take_new_questions().is_empty());
    }
}
