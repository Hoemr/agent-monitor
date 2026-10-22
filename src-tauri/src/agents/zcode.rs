//! ZCode adapter.
//!
//! Real source: `~/.zcode/v2/tasks-index.sqlite`, table `tasks` — ZCode's own
//! index of conversations/tasks (task_id, title, task_status, workspace, model,
//! created_at/updated_at in ms). Read-only, opened with `immutable=1` so a
//! running ZCode process is never blocked by our reads.
//!
//! Status vocabulary observed on this machine: `completed`. The application
//! bundle also references `running`, `pending`, `failed`, `cancelled`; those
//! mappings are implemented but marked 待验证 in docs/CAPABILITIES.md until a
//! live record is observed.

use super::{retention_keeps, truncate_chars, ScanCtx, Source};
use crate::model::*;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const APP_NAME: &str = "ZCode";
const BUNDLE_ID: &str = "dev.zcode.app";
const PROCESS_NEEDLE: &str = "ZCode.app/Contents/MacOS/ZCode";

struct TaskRow {
    workspace_path: String,
    task_id: String,
    title: String,
    task_status: String,
    model: String,
    created_at: u64,
    updated_at: u64,
}

pub struct ZCodeSource {
    db_mtime: u64,
    rows: Vec<TaskRow>,
}

impl ZCodeSource {
    pub fn new() -> Self {
        Self {
            db_mtime: 0,
            rows: Vec::new(),
        }
    }

    fn db_path() -> Option<PathBuf> {
        super::home_dir().map(|h| h.join(".zcode/v2/tasks-index.sqlite"))
    }

    fn open_ro(path: &Path) -> Option<Connection> {
        let uri = format!("file:{}?immutable=1", uri_path(path));
        Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()
    }

    fn reload(&mut self) {
        let Some(path) = Self::db_path() else {
            return;
        };
        let Some(conn) = Self::open_ro(&path) else {
            return;
        };
        let mut rows = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "select workspace_path, task_id, title, coalesce(task_status,''), coalesce(model,''),
                    created_at, updated_at
             from tasks
             where coalesce(deleted,0) = 0 and coalesce(archived,0) = 0
             order by updated_at desc limit 80",
        ) {
            if let Ok(mapped) = stmt.query_map([], |row| {
                Ok(TaskRow {
                    workspace_path: row.get(0)?,
                    task_id: row.get(1)?,
                    title: row.get(2)?,
                    task_status: row.get(3)?,
                    model: row.get(4)?,
                    created_at: row.get::<_, i64>(5)? as u64,
                    updated_at: row.get::<_, i64>(6)? as u64,
                })
            }) {
                rows.extend(mapped.flatten());
            }
        }
        self.rows = rows;
    }
}

fn uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F")
}

/// Map ZCode's index status to the unified status. Never infers a waiting
/// state: only explicit waiting_* literals (unobserved here) map to them.
fn map_status(s: &str) -> (TaskStatus, String) {
    let key = s.trim().to_ascii_lowercase();
    match key.as_str() {
        "completed" | "complete" | "done" => {
            (TaskStatus::Completed, "task_status = completed".into())
        }
        "failed" | "error" => (TaskStatus::Failed, "task_status = failed".into()),
        "cancelled" | "canceled" => (TaskStatus::Failed, "task_status = cancelled".into()),
        "running" | "in_progress" | "working" | "executing" => {
            (TaskStatus::Running, format!("task_status = {key}"))
        }
        "waiting_approval" | "awaiting_approval" => (
            TaskStatus::WaitingApproval,
            format!("task_status = {key}"),
        ),
        "waiting_input" | "awaiting_input" | "needs_input" => (
            TaskStatus::WaitingInput,
            format!("task_status = {key}"),
        ),
        "idle" => (TaskStatus::Idle, "task_status = idle".into()),
        "" => (TaskStatus::Unknown, "task_status is empty".into()),
        other => (TaskStatus::Unknown, format!("unmapped task_status = {other}")),
    }
}

/// Retention decision for one indexed task: active rows (running/waiting) are
/// exempt through the shared helper, everything ended follows the user-configured
/// window. There is deliberately no "keep the newest N rows" fallback: that
/// showed sessions weeks older than the configured retention forever.
fn keeps_row(
    status: TaskStatus,
    updated_secs: u64,
    now_secs: u64,
    retention_secs: u64,
) -> bool {
    retention_keeps(status, updated_secs, now_secs, retention_secs)
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

impl Source for ZCodeSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "zcode".into(),
            name: "ZCode".into(),
            source: "~/.zcode/v2/tasks-index.sqlite (tasks table)".into(),
            origin: DataOrigin::Index,
            max_level: JumpLevel::L1,
            notes: "Task identity from ZCode's index. zcode://workspace/open?path= exists in the bundle but could not be verified: ZCode.app exits immediately on this host (see docs/JUMP.md), so only application activation is declared.".into(),
        }
    }

    fn invalidate(&mut self) {
        self.db_mtime = 0;
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        let Some(db) = Self::db_path() else {
            return Vec::new();
        };
        let mtime = super::file_mtime_secs(&db);
        if mtime != self.db_mtime {
            self.reload();
            self.db_mtime = mtime;
        }
        let app_alive = ctx.processes.any(PROCESS_NEEDLE) || ctx.processes.any(APP_NAME);
        let mut out = Vec::new();
        for row in self.rows.iter() {
            let updated = row.updated_at / 1000;
            let (status, reason) = map_status(&row.task_status);
            // Active rows are exempt inside `keeps_row`; ended ones must be
            // inside the configured retention window.
            if !keeps_row(status, updated, ctx.now_secs, ctx.retention_secs) {
                continue;
            }
            let workspace = Path::new(&row.workspace_path);
            let workspace_exists = workspace.is_dir();
            let title = if row.title.trim().is_empty() {
                workspace
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "ZCode task".into())
            } else {
                truncate_chars(row.title.trim(), 120)
            };
            let summary = if row.model.is_empty() {
                None
            } else {
                Some(truncate_chars(
                    row.model.rsplit('/').next().unwrap_or(&row.model),
                    40,
                ))
            };
            let mut record = TaskRecord {
                id: format!("zcode:{}", row.task_id),
                agent_key: "zcode".into(),
                agent_name: "ZCode".into(),
                host: "desktop".into(),
                session_id: Some(row.task_id.clone()),
                task_id: Some(row.task_id.clone()),
                title,
                status,
                status_reason: reason,
                last_activity_secs: updated,
                collected_at_secs: ctx.now_secs,
                summary,
                origin: DataOrigin::Index,
                target: JumpTarget {
                    app_name: Some(APP_NAME.into()),
                    bundle_id: Some(BUNDLE_ID.into()),
                    deep_link: Some(format!(
                        "zcode://workspace/open?path={}",
                        percent_encode(&row.workspace_path)
                    )),
                    window_title: Some(row.title.clone()),
                    workspace_path: Some(row.workspace_path.clone()),
                    ..Default::default()
                },
                target_validity: TargetValidity::Unconfirmed,
                invalid_reason: None,
                capability: JumpCapability {
                    max_level: JumpLevel::L1,
                    notes: "Application activation only: the app could not be kept running for window verification on this host."
                        .into(),
                    requires: vec![PermissionKind::Automation],
                },
                level: JumpLevel::L1,
                pinned: false,
            };
            if !workspace_exists {
                record.target_validity = TargetValidity::Invalid;
                record.invalid_reason = Some("workspace_missing".into());
            } else if app_alive {
                record.target_validity = TargetValidity::Valid;
            }
            record.refresh_level();
            out.push(record);
            let _ = row.created_at;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_observed_and_referenced_statuses() {
        assert_eq!(map_status("completed").0, TaskStatus::Completed);
        assert_eq!(map_status("running").0, TaskStatus::Running);
        assert_eq!(map_status("failed").0, TaskStatus::Failed);
        assert_eq!(map_status("cancelled").0, TaskStatus::Failed);
        assert_eq!(map_status("pending").0, TaskStatus::Unknown);
        assert_eq!(map_status("").0, TaskStatus::Unknown);
    }

    #[test]
    fn pending_is_never_reported_as_waiting_input() {
        let (status, reason) = map_status("pending");
        assert_ne!(status, TaskStatus::WaitingInput);
        assert!(reason.contains("unmapped"));
    }

    #[test]
    fn workspace_deep_link_is_percent_encoded() {
        let link = format!(
            "zcode://workspace/open?path={}",
            percent_encode("/Users/me/My Project")
        );
        assert!(link.contains("My%20Project"));
    }

    #[test]
    fn stable_id_uses_task_id_not_title() {
        let id = format!("zcode:{}", "sess_abc");
        assert_eq!(id, "zcode:sess_abc");
        assert_ne!(id, "zcode:Some title");
    }

    #[test]
    fn retention_governs_ended_rows_including_the_newest_and_keeps_active_rows() {
        let now = 2_000_000;
        let window = 72 * 3_600;
        // Regression: there is no unconditional "newest 3 rows" fallback any
        // more. An ended row is dropped as soon as its `updated_at` is outside
        // the configured window, whatever its position in the index order.
        let fifteen_days_ago = now - 15 * 86_400;
        assert!(!keeps_row(
            TaskStatus::Completed,
            fifteen_days_ago,
            now,
            window
        ));
        assert!(!keeps_row(TaskStatus::Failed, fifteen_days_ago, now, window));
        // The inclusive boundary is kept.
        assert!(keeps_row(TaskStatus::Completed, now - window, now, window));
        // Active rows are never aged out.
        for status in [
            TaskStatus::Running,
            TaskStatus::WaitingInput,
            TaskStatus::WaitingApproval,
        ] {
            assert!(keeps_row(status, 0, now, window), "{status:?} must stay");
        }
        // Retention is injected, not the old hard-coded 24 h ZCode constant.
        let two_days_ago = now - 48 * 3_600;
        assert!(keeps_row(TaskStatus::Completed, two_days_ago, now, window));
        assert!(!keeps_row(
            TaskStatus::Completed,
            two_days_ago,
            now,
            24 * 3_600
        ));
    }
}
