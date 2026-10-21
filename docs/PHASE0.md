# Phase 0 — Repository inspection, data flow, and capability baseline

Date: 2026-10-21. Host: macOS 27.0.1 (arm64, single 1920x1080 display), Swift 6.4,
Command Line Tools only (no Xcode.app), Rust 1.99.1 installed via rustup, Node 24,
`gh` authenticated as the repository owner.

## 1. What the existing project already does

`Hoemr/agent-monitor` v0.3.1 is a **Tauri 2 + Rust + vanilla JS** always-on-top desktop widget.
It is not a Swift/AppKit app, so the refactor stays inside Tauri.

Existing capabilities (verified by reading the source):

| Area | Implementation | File |
|---|---|---|
| Data source | Claude Code only: `~/.claude/projects/**/*.jsonl`, `file-history/` mtimes, `ide/*.lock` | `src-tauri/src/lib.rs` |
| Parsing | Incremental JSONL reader with byte offset + mtime cache, status from last `tool_use` / `stop_reason` / 10 s silence | `src-tauri/src/lib.rs` |
| Refresh | `notify::PollWatcher` (2 s) on `projects/`, `file-history/`, `ide/` + frontend fallback poll | `lib.rs`, `src/main.js` |
| UI | Project folders with nested session rows, pixel style, footer counters, 1 s clock | `src/index.html`, `src/main.js`, `src/style.css` |
| Window | One borderless always-on-top window, click-through toggle, 3 preset heights | `lib.rs`, `tauri.conf.json` |
| Tray | Show / Refresh / Preferences / Click-through / Quit; left-click toggles window | `lib.rs` |
| Click action | Every row calls `open_in_vscode` → `code --goto <file> <project>`; **no per-session targeting** | `lib.rs`, `src/main.js` |
| Persistence | `localStorage` only (language, poll interval, opacity, size) | `src/main.js` |
| Tests | **None** | — |
| Permissions | None declared; no accessibility / automation usage | — |

Gaps relative to this task: single Agent, application-level click behavior, no
unified model, no jump grading/degradation, no desktop-pet geometry, no
multi-display handling, no tests, no performance evidence.

## 2. Real data sources found on this machine

All four required objects have a **real, local, inspectable** source. No mocked
data is used for them; a synthetic adapter exists only for the extension test.

| Agent | Real source (verified) | Multi-session key | Status evidence available |
|---|---|---|---|
| **Codex** | `~/.codex/state_5.sqlite` table `threads` (192 rows) + `rollout_path` JSONL under `~/.codex/sessions/YYYY/MM/DD/` + `~/.codex/thread_history_1.sqlite` (`thread_turns.status`: completed/failed/interrupted) | `threads.id` (UUID) | last rollout `event_msg` = `task_started` → running; `task_complete` → done; `thread_turns.status` → failed/interrupted; no file writes → idle |
| **DSH (DeepSeek Harness)** | `~/.dsh/storages/session_projcache/sessions/*.json` (title, cwd, `turnBoundary`, `userQuestions`, `sessionStats`) + `~/.dsh/sessions/<cwd>/<id>/session.v4.jsonl.zstd` + `session.lock` | session dir name (`session-<uuid>` or `<uuid>`) | `turnBoundary.openTurnStartSeq != null` → running; `lastStepBoundary.kind`; `userQuestions.questions.active` → waiting input; `turn/end` in JSONL → completed |
| **ZCode** | `~/.zcode/v2/tasks-index.sqlite` table `tasks` (22 rows: `task_id`, `title`, `task_status`, `updated_at`, `workspace_path`, `model`) | `tasks.task_id` = `sess_<uuid>` | `task_status` (`completed` observed; app bundle also references `running`/`pending`/`failed`/`cancelled`) |
| **VSCode agent plugin** | **Claude Code extension** `anthropic.claude-code` 2.1.304: `~/.claude/projects/**/*.jsonl`, `~/.claude/file-history/`, `~/.claude/ide/*.lock` (PID + workspaceFolders). Codex VSCode extension `openai.chatgpt` also exists and is exposed as a Codex host. | Claude session UUID / Codex thread id | Last assistant block + `stop_reason`; `ide/*.lock` PID liveness; silence heuristic only as *unknown*, never as waiting |

Observed vocabulary (not assumed):

* Codex `thread_turns.status`: `completed`, `failed`, `interrupted`. Turn history is
  written when a turn ends, so it never contains `running`; live running is read from
  the rollout tail (`task_started` without a following `task_complete`).
* DSH event types: `turn/start`, `turn/end`, `step/start`, `step/end`, `tool/call`,
  `tool/result`, `session/title`, `permission/preset`, `approval/policy`,
  `user/message`, `session/end-seed`. Approval *requests* were not observed
  (policy `never` on this machine) → DSH waiting-approval is not claimed.
* ZCode: only `completed` rows exist right now; `running`/`pending` strings exist in
  the app bundle but were **not observed in data** → mapping implemented, marked
  "待验证".

## 3. Hosts and jump candidates (to be verified in Phase 2)

| Agent | Host | Bundle id | Verified so far |
|---|---|---|---|
| Codex | ChatGPT desktop app | `com.openai.codex` | Running; AX window title = thread title (“编写 Agent 桌宠实现 Prompt”); deep link `codex://threads/<id>` exists in bundle |
| Codex | VSCode extension `openai.chatgpt` | `com.microsoft.VSCode` | Extension activates `onUri` |
| DSH | DeepSeek Harness app | `com.deepseek.dsh` | Running; AX window title = session title; `dsh://open/...` route exists in bundle |
| ZCode | ZCode app | `dev.zcode.app` | Not running during inspection; `zcode://workspace/open?path=` exists in bundle |
| Claude Code | VSCode extension `anthropic.claude-code` | `com.microsoft.VSCode` | Commands + webview panels; no URI handler in `activationEvents` → no L3 claim yet |
| any CLI | Terminal.app | `com.apple.Terminal` | AX/Automation working from probe shell; tab `tty` enumerable |

Verified probe environment facts:
* `osascript` + System Events can read window titles for ChatGPT and DeepSeek Harness (Accessibility/Automation already granted to the probe shell).
* `open -b <bundle>` activation works.
* Only one display (`1920x1080`, origin 0,0) → physical multi-monitor tests cannot run here and will be marked **未执行** with reproducible steps; geometry is covered by unit tests.
* No `claude` CLI installed; `dsh` CLI lives inside the app bundle; `codex` binary lives inside ChatGPT.app. The legacy Claude `open_in_vscode` used `code --goto`, which only opens a file — it does not select a session.

## 4. Shortest data flow

```
adapter.scan(now)  ──►  Vec<TaskRecord>            (one per real session/task)
   │  std-only file/sqlite readers, incremental, no per-task timers
   ▼
monitor: merge → stable id dedupe → status/validity normalise
   │  only emit when the serialised snapshot changes (hash)
   ▼
Tauri emit "tasks-updated"  ──►  pet UI (one unified task list)
   │                                   │
   │                                   └─ click → invoke jump_to_task(id)
   ▼
tray (recall/reset)                   jump.rs: resolve level L3→L0,
                                      execute, verify, return JumpOutcome
                                      (degrade + short inline feedback)
```

Timing: event-driven via `notify` watchers on the four data roots, debounced
(700 ms). A 5 s tick only re-scans adapters whose backing files' mtimes changed;
when the pet is hidden or there are no tasks the tick stretches to 30 s and no UI
event is emitted. No 1 Hz render loop (relative times are refreshed by one shared
30 s timer that only rewrites time text).

## 5. Files to change (Phase 1) and later phases

New:
```
src-tauri/src/model.rs            unified task/jump/agent types
src-tauri/src/agents/mod.rs       Source trait + registry
src-tauri/src/agents/claude_code.rs
src-tauri/src/agents/codex.rs
src-tauri/src/agents/dsh.rs
src-tauri/src/agents/zcode.rs
src-tauri/src/agents/simulated.rs  extension-test adapter (clearly labelled)
src-tauri/src/monitor.rs          collect/merge/dedupe/emit
src-tauri/src/jump.rs             graded jump + verification + feedback
src-tauri/src/pet.rs              geometry, snap, edge hide, persistence
docs/CAPABILITIES.md              capability matrix (updated per phase)
docs/ACCEPTANCE.md                per-phase results + evidence
docs/PERF.md                      measurement environment + numbers
docs/NEW_AGENT.md                 how to add an agent
scripts/perf.sh                   sampling harness
```
Modified: `src-tauri/src/lib.rs` (replace legacy scanner/click with monitor+jump+pet wiring),
`src-tauri/src/main.rs` (unchanged), `src-tauri/Cargo.toml` (+ rusqlite), `src-tauri/tauri.conf.json`
(window: transparent, no decorations, always-on-top, skip taskbar, focus:false, macOS bundle targets),
`src-tauri/capabilities/default.json` (tray/menu/window permissions if needed),
`src/index.html`, `src/main.js`, `src/style.css` (pet UI), `package.json` (dev/build scripts for macOS),
`README.md` / `README_ZH.md` (build + run + capability pointers).

Removed after replacement: the legacy `parse_jsonl`/`scan_all_sessions`/`open_in_vscode`
path in `lib.rs` (logic moves to `agents/claude_code.rs`), the project-folder DOM renderer,
and the 1 s clock interval.

## 6. Original monitoring regression checklist

Kept from the old implementation (must still pass after refactor):

- [ ] Claude Code session discovered from `~/.claude/projects/**/*.jsonl` within 2 poll cycles.
- [ ] Two parallel Claude sessions in the same project appear as **two** task bars, never merged.
- [ ] Re-scan with no file change does not create duplicate records (stable ids).
- [ ] `tool_use` → running with tool name; `end_turn` → idle/completed.
- [ ] `/clear`-style new JSONL in a reused session re-enters running.
- [ ] `file-history` mtime keeps a long-thinking session from flipping to done within 120 s.
- [ ] `ide/*.lock` PID liveness marks VSCode-hosted sessions as active; dead PID ignored.
- [ ] Token/model/message metadata still surfaced in the row detail (compact form).
- [ ] `subagent` JSONL files excluded.
- [ ] Stale (>30 min, not IDE) sessions hidden.
- [ ] Tray: show/hide, refresh, quit still work; single-instance still focuses the pet.
- [ ] Click-through toggle still works.
- [ ] Language switch (en/zh) still works for the visible labels.
- [ ] No secrets/prompt bodies persisted; only short titles/previews.

## 7. Verified facts vs. assumptions

Verified: data sources and schemas above; AX window-title matching for ChatGPT and
DSH; `open -b` activation; URL schemes present in bundles; single display;
Accessibility granted to probe shell; no `claude` CLI.

Assumptions / pending verification:
1. `codex://threads/<id>` opens that thread (to test in Phase 2).
2. `dsh://open/<session-id>` selects that session (route exists, parameter shape to test).
3. ZCode window title exposes the task title after `zcode://workspace/open?path=` (app not running yet).
4. VSCode can be raised to the correct window by workspace-name title match (test with a real VSCode window).
5. Accessibility/Automation permission granted by the user to the packaged app (probe shell permission does not transfer).
6. Multi-display / Space / full-screen behavior cannot be physically tested on this single-display host → will be documented as 未执行 with steps.

---

## 8. Corrections after implementation (facts that changed the plan)

1. **DSH has no session deep link.** Its Electron main process only handles
   `dsh://open` / `dsh://open/` → `focusPrimaryWindow()`. The earlier assumption
   that `dsh://open/<id>` might select a session is false; DSH is therefore
   capped at **L1** and the matrix says so. (`docs/JUMP.md` #3/#4)
2. **Codex window titles are AI-generated.** `threads.title` stores the first user
   prompt while the window shows a short generated title, so exact-title matching
   cannot work for Codex. L3 is verified by a real navigation (window title change
   before/after the `codex://threads/<id>` open), not by comparing the stored title.
3. **ZCode cannot be verified on this host.** `ZCode.app` (Electron 41, macOS 27
   beta) exits immediately with status 0, no window and no crash report. Its data
   source works; its jump is declared L1 and reports unconfirmed instead of
   pretending. This is a documented blocker, not an implementation omission.
4. **No `claude` CLI is installed**, so Terminal tab-level (L3) selection is
   implemented but 未验证; Terminal application activation (L1) is verified.
5. **macOS ignores `set_position` on an unmapped window.** The setup order had to
   be: show → position → apply mode. A read-back immediately after a move is also
   stale, so the app keeps an authoritative `last_rect` instead of re-reading.
   This was found during geometry verification (72/360/edge-hidden bounds).
6. **ZCode workspace path** is a VS Code-like workspace; the adapter opens the
   SQLite index read-only with `immutable=1`, which works while the app writes.
7. **DSH projcache is the cheap authoritative status source**; no zstd
   decompression is used in scans (bounded CPU, no new dependency). Sessions with
   no projected state fall back to `Unknown` with a reason.
8. **Retention policy** was added after seeing 73 mostly-old records: active tasks
   are always shown, finished work is retained for 6 h (24 h for IDE-matched
   transcripts), and ZCode keeps its newest 3 real tasks so the agent stays
   represented without unbounded history. This is a display policy, not a data
   deletion.
9. **Frontend contract changed in two places** vs the draft: `jump_to_task` returns
   `Result<JumpOutcome, String>` (Tauri async-command requirement), and a
   mock-only `__AM_TEST__` hook exists strictly under `?mock=…` for the headless UI
   acceptance; the packaged app never exposes it.

10. **Follow-up product changes (after first delivery):** the app is now
    **tray-only** (accessory activation policy; Dock icon hidden, also for
    `cargo run`), the pixel webfont was replaced with the macOS system font stack,
    Claude Code is modelled strictly as the **VSCode extension host** (no Terminal
    routing), and the task list is sorted by **newest activity first** instead of
    status priority. The status-priority ordering was removed from `sort_tasks`;
    waiting/failed rows are still surfaced by the badge, row accent and glyph.
11. **Accessory policy must be re-asserted after window mapping.** Setting it only
    in `setup` was silently promoted back to `.regular` by AppKit during first
    activation; a delayed re-assert (~250 ms and ~1.2 s) makes
    `NSRunningApplication.activationPolicy == .accessory` stable and removes the
    Dock icon in both the `.app` and the bare binary.
