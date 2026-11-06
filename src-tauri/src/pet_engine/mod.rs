//! Rust desktop-pet engine.
//!
//! The pet system proper: loads Shimeji-ee-compatible mascot packs
//! ([`pack`]) and runs their behavior state machine ([`behavior`]) — walk,
//! fall, drag, throw, weighted scheduling — entirely in Rust. The frontend is
//! a thin blit layer: every tick the engine produces a [`behavior::Frame`]
//! (image + anchor + facing) that the pet window draws.
//!
//! The monitor's task stream feeds [`mood_from_tasks`], so the mascot reacts
//! to agent activity: works while tasks run, celebrates completions, tugs
//! attention on pending questions, mopes over recent failures. See
//! docs/PETS.md for the supported pack subset and extension points.

pub mod backend;
pub mod behavior;
pub mod mascot;
pub mod pack;
pub mod state;

use crate::model::{TaskRecord, TaskStatus};
use behavior::Mood;

/// Derive the pet's mood from the current task snapshot. Priority follows
/// the attention rules of the monitor: a pending question outranks a recent
/// failure, which outranks active work; a *recent* completion celebrates
/// only when nothing more pressing is going on.
pub fn mood_from_tasks(tasks: &[TaskRecord], now_secs: u64) -> Mood {
    let recent = |secs: u64| now_secs.saturating_sub(secs) <= 30 * 60;
    let mut mood = Mood::Idle;
    for task in tasks {
        match task.status {
            TaskStatus::WaitingInput | TaskStatus::WaitingApproval => return Mood::Attention,
            TaskStatus::Failed if task.needs_attention_at(now_secs) => mood = Mood::Distressed,
            TaskStatus::Running if mood != Mood::Distressed => mood = Mood::Working,
            TaskStatus::Completed
                if recent(task.last_activity_secs) && mood == Mood::Idle =>
            {
                mood = Mood::Celebrate
            }
            _ => {}
        }
    }
    mood
}

/// Keeps celebration tied to a new final result rather than a lingering row.
/// Updated even while the task panel is open or the pet is hidden.
#[derive(Default)]
pub struct TaskMoodTracker {
    completions: std::collections::BTreeSet<(String, String)>,
    initialized: bool,
    celebrate_until_ms: u64,
}

impl TaskMoodTracker {
    pub fn update(&mut self, tasks: &[TaskRecord], now_ms: u64) -> Mood {
        let mut current = std::collections::BTreeSet::new();
        let mut fresh = false;
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Completed) {
            let key = (task.id.clone(), task.completion_id.clone()
                .unwrap_or_else(|| task.last_activity_secs.to_string()));
            if self.initialized && !self.completions.contains(&key)
                && now_ms.saturating_sub(task.last_activity_secs.saturating_mul(1000)) <= 30_000
            {
                fresh = true;
            }
            current.insert(key);
        }
        self.completions = current;
        self.initialized = true;
        if fresh { self.celebrate_until_ms = now_ms.saturating_add(3_000); }
        let mood = mood_from_tasks(tasks, now_ms / 1000);
        match mood {
            Mood::Attention | Mood::Distressed => mood,
            _ if now_ms < self.celebrate_until_ms => Mood::Celebrate,
            Mood::Celebrate => Mood::Idle,
            _ => mood,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CompletionEvidence, DataOrigin, JumpLevel, TargetValidity};

    fn task(status: TaskStatus, age_secs: u64) -> TaskRecord {
        TaskRecord {
            id: "t".into(),
            agent_key: "zcode".into(),
            agent_name: "ZCode".into(),
            host: "desktop".into(),
            session_id: None,
            task_id: None,
            title: "t".into(),
            status,
            completion_id: None,
            completion: (status == TaskStatus::Completed).then_some(CompletionEvidence::TaskIndex),
            status_reason: String::new(),
            last_activity_secs: 10_000 - age_secs,
            collected_at_secs: 10_000,
            summary: None,
            model: None,
            origin: DataOrigin::Index,
            target: Default::default(),
            target_validity: TargetValidity::Unconfirmed,
            invalid_reason: None,
            capability: crate::model::JumpCapability {
                max_level: JumpLevel::L1,
                notes: String::new(),
                requires: Vec::new(),
            },
            level: JumpLevel::L1,
            pinned: false,
        }
    }

    #[test]
    fn persistent_attention_and_failures_follow_panel_rules() {
        let mut tracker = TaskMoodTracker::default();
        let waiting = task(TaskStatus::WaitingApproval, 5);
        assert_eq!(tracker.update(&[waiting.clone()], 10_000_000), Mood::Attention);
        assert_eq!(tracker.update(&[waiting], 10_010_000), Mood::Attention);
        let mut failure = task(TaskStatus::Failed, 5);
        assert_eq!(tracker.update(&[failure.clone()], 10_011_000), Mood::Distressed);
        assert_eq!(tracker.update(&[failure.clone()], 10_020_000), Mood::Distressed);
        failure.status_reason = "turn aborted by user".into();
        assert_eq!(tracker.update(&[failure], 10_021_000), Mood::Idle);
    }

    #[test]
    fn completion_identity_celebrates_once_and_new_turn_can_celebrate_again() {
        let mut tracker = TaskMoodTracker::default();
        let mut completed = task(TaskStatus::Completed, 0);
        completed.completion_id = Some("answer-1".into());
        // Opening the app must not celebrate the history already on the panel.
        assert_eq!(tracker.update(&[completed.clone()], 10_000_000), Mood::Idle);
        completed.completion_id = Some("answer-2".into());
        assert_eq!(tracker.update(&[completed.clone()], 10_001_000), Mood::Celebrate);
        assert_eq!(tracker.update(&[completed.clone()], 10_002_000), Mood::Celebrate);
        assert_eq!(tracker.update(&[completed.clone()], 10_004_000), Mood::Idle);
        // Unrelated timestamp changes do not create a new final result.
        completed.last_activity_secs = 10_005;
        assert_eq!(tracker.update(&[completed.clone()], 10_005_000), Mood::Idle);
        completed.completion_id = Some("answer-3".into());
        assert_eq!(tracker.update(&[completed.clone()], 10_006_000), Mood::Celebrate);
        assert_eq!(tracker.update(&[completed.clone(), task(TaskStatus::Running, 0)], 10_006_500), Mood::Celebrate);
        assert_eq!(tracker.update(&[completed.clone(), task(TaskStatus::Running, 0)], 10_009_000), Mood::Working);
        assert_eq!(tracker.update(&[completed, task(TaskStatus::WaitingInput, 0)], 10_007_000), Mood::Attention);
    }

    #[test]
    fn task_snapshot_drives_real_state_frames_through_entire_lifecycle() {
        use super::backend::{PetBackend, PetFrame};
        let files = state::builtin_state_pack_files();
        let (_, files) = files.iter().find(|(name, _)| *name == "british-blue").unwrap();
        let mut backend = state::StateBackend::new(state::load_state_pack("british-blue", files).unwrap());
        let mut tracker = TaskMoodTracker::default();
        for (status, time, expected) in [
            (TaskStatus::Running, 10_000_000, "working"),
            (TaskStatus::WaitingInput, 10_001_000, "watching"),
            (TaskStatus::Running, 10_002_000, "working"),
            (TaskStatus::Failed, 10_003_000, "alarmed"),
            (TaskStatus::Running, 10_004_000, "working"),
            (TaskStatus::Completed, 10_005_000, "celebrating"),
            (TaskStatus::Completed, 10_008_000, "idle"),
        ] {
            backend.set_mood(tracker.update(&[task(status, 0)], time));
            assert!(matches!(backend.tick(), Some(PetFrame::State { state }) if state == expected), "{status:?} should show {expected}");
        }
    }

    #[test]
    fn attention_outranks_everything() {
        let tasks = vec![task(TaskStatus::Running, 5), task(TaskStatus::WaitingInput, 5)];
        assert_eq!(mood_from_tasks(&tasks, 10_000), Mood::Attention);
    }

    #[test]
    fn recent_failure_beats_running() {
        let tasks = vec![task(TaskStatus::Running, 5), task(TaskStatus::Failed, 60)];
        assert_eq!(mood_from_tasks(&tasks, 10_000), Mood::Distressed);
        // Old failures no longer mope (same rule as the attention badge).
        let tasks = vec![task(TaskStatus::Running, 5), task(TaskStatus::Failed, 4_000)];
        assert_eq!(mood_from_tasks(&tasks, 10_000), Mood::Working);
    }

    #[test]
    fn running_beats_recent_completion_and_idle_when_quiet() {
        let tasks = vec![task(TaskStatus::Running, 5), task(TaskStatus::Completed, 10)];
        assert_eq!(mood_from_tasks(&tasks, 10_000), Mood::Working);
        let tasks = vec![task(TaskStatus::Completed, 10)];
        assert_eq!(mood_from_tasks(&tasks, 10_000), Mood::Celebrate);
        let tasks = vec![task(TaskStatus::Completed, 4_000)];
        assert_eq!(mood_from_tasks(&tasks, 10_000), Mood::Idle);
        assert_eq!(mood_from_tasks(&[], 10_000), Mood::Idle);
    }
}
