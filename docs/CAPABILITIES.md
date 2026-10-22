# Capability matrix (verified on this Mac)

Environment: macOS 27.0.1 (26A434), arm64, single display 1920×1080 @ scale 1,
Rust 1.99.1, release build (`cargo build --release`, Tauri 2.11 + custom-protocol +
macOSPrivateApi). Date: 2026-10-21.

Legend: **L3** session/tab, **L2** window, **L1** application, **L0** unavailable.
“Verified” means a real action was executed and the result was read back
(see `docs/JUMP.md` for commands and observed evidence). Declared capability is
never higher than the verified level.

| Object | Real data source (read-only) | Multi-session identity | Highest verified jump | Required permission | Degradation behaviour | Test result |
|---|---|---|---|---|---|---|
| **Codex** (ChatGPT app + VSCode extension `openai.chatgpt`) | `~/.codex/state_5.sqlite` `threads` (192 rows) + rollout JSONL tail + `thread_history_1.sqlite` `thread_turns` | `threads.id` (UUID); 61 threads were created in the VSCode extension, rest in the app | **L3 — verified**: `codex://threads/<id>` changes the visible thread (window title changed from `编写 Agent 桌宠实现 Prompt` → `调研密度比估计与扩散生成`, outcome `success/L3`) | Automation (used only to read back the window title); Accessibility for AXRaise fallback | Deep link unverified or app not running → L2 window match → L1 `open -b com.openai.codex` → toast; only the app is activated when the session cannot be proven | **Pass** (real record + real L3) |
| **DSH** (DeepSeek Harness desktop) | `~/.dsh/storages/session_projcache/sessions/*.json` (67 sessions: title, cwd, `turnBoundary`, `userQuestions`, stats) | Session directory name (`session-<uuid>`); sidebar groups sessions by workspace; 3 parallel sessions were visible at the same time in the live run | **L3 — verified (round 2)**: the host has no session URL (`dsh://open` only focuses the app), but the sidebar is exposed through the native macOS AX API; matching the row on session title + project (cwd basename) + relative time and clicking it (CGEvent) switches the window — verified by the focused window title `<session> — DeepSeek Harness` (switched away and back on 2026-10-22). | Accessibility (AX tree + synthetic click); Automation not needed for this route | Sidebar row not rendered (virtualized list) → app focus with a precise reason; duplicate titles → project/time evidence, otherwise `session_ambiguous` instead of a wrong click; already-visible session → success without clicking | **Pass** (real record + real L3 with read-back) |
| **ZCode** (ZCode desktop) | `~/.zcode/v2/tasks-index.sqlite` `tasks` (22 tasks, status/model/workspace) | `tasks.task_id` (`sess_<uuid>`) | **L1 declared, blocked in verification**: `open -b dev.zcode.app` returns 0 but `ZCode.app` exits immediately (Electron 41; exit code 0, no window, no crash report) → activation cannot be confirmed. `zcode://workspace/open?path=` exists in the bundle but is unverified | Automation | Attempts workspace deep link/window (declared L1 so it stops at L1) → reports `focus_unconfirmed`/`activated_app_only`; never pretends to reach a task | **Partial** (real record; jump blocked — documented) |
| **VSCode agent plugin** — Claude Code `anthropic.claude-code` 2.1.304 | `~/.claude/projects/**/*.jsonl` + `file-history/` + `ide/*.lock` (ported legacy scanner) | Claude transcript UUID; multiple parallel sessions per project stay separate | **L2 — verified (round 2, project-precise)**: `code --reuse-window <workspace_path>` focuses the already-open window for that project even with several VSCode windows open (verified with two windows: target window moved to front and its title matched the workspace), then AX focused-window title read-back; AX raise remains the fallback. | Accessibility + (Automation for the AppleScript fallback); the `code` CLI is located in PATH/Homebrew/`/usr/local/bin` | No `code` CLI → `open -b com.microsoft.VSCode <workspace>` → AX window title match (`<file> — <folder>`) → L1 activate VSCode; the extension panel itself is not scriptable, so session-level is not claimed | **Pass** (real record + real L2) |
| **Simulated Agent** (extension acceptance only, `AGENT_MONITOR_SIMULATED=1`) | In-process fixture, `origin = synthetic`, UI shows `SIM` | 3 fixture sessions | L1 fixture | none | Clearly labelled, never mixed with real adapters | **Pass** (see ACCEPTANCE §Phase 4) |

## Status mapping and evidence

| Unified status | Codex | DSH | ZCode | Claude Code | Simulated |
|---|---|---|---|---|---|
| Running | rollout last `event_msg = task_started` (app alive, <30 m silence) | `turnBoundary.openTurnStartSeq != null` (+ `lastStepBoundary.kind`), app alive | `task_status ∈ {running, in_progress, working, executing}` (**not observed live** — 待验证) | `tool_use` / `thinking` / `responding` in transcript | fixture |
| Waiting input | not available (no explicit signal) | `userQuestions.questions.active[]` non-empty (**explicit**) | `waiting_input/needs_input` literals (**not observed**) | not available — silence is never used | fixture |
| Waiting approval | not available | not available (policy `never`, no request event observed) | `waiting_approval` literal (**not observed**) | not available | fixture |
| Completed | `task_complete` or latest `thread_turns.status = completed` | open turn closed, `lastStepBoundary.kind = end` | `completed` | `stop_reason end_turn` / silence rule | fixture |
| Failed | `thread_turns.status ∈ {failed, interrupted}` | — | `failed`, `cancelled` | — | fixture |
| Idle | thread with no rollout/turns | session with no turns | `idle` | — | — |
| Unknown | no state event / app died with open turn / >30 m silence | open turn but app not running / >30 m silence / no boundary state | empty or unmapped status | unmapped legacy status | — |

Rules enforced by tests: waiting states are only produced from explicit
signals; “no output” never becomes waiting; completed + closed window is shown as
completed **plus** an invalid target, not as failed; a reused session can return
to running.

## Ordering and presentation

The task list is sorted by **`last_activity_secs` descending** (newest first);
status only breaks ties. This was changed after user feedback: the live/most
recent conversation must be the first row. Waiting/approval/failure are surfaced
by the count badge, the row accent bar and the status glyph — the list is **not**
grouped or re-ordered by state.

The UI uses the macOS system font stack (SF Pro / PingFang SC) — the pixel webfont
was removed. The app runs with an **accessory activation policy**: menu-bar/tray
only, no Dock icon.

## Known limitations / 未执行

* **Multi-display physical test**: this host has one display. Geometry is covered by
  unit tests for negative coordinates, stacked displays, screen removal, resolution
  change, and Dock/menu-bar visible-frame avoidance, but a second physical display
  and hot-plug were **not executed**. Repro steps in `docs/ACCEPTANCE.md`.
* **Space / full-screen / Stage Manager**: the pet is set to
  `visibleOnAllWorkspaces`, so it follows Spaces; behaviour over a full-screen app
  was **not executed** (requires a full-screen app and visual check).
* **Permission prompts**: the probe shell already had Automation/Accessibility.
  The packaged app’s first-run TCC dialogs were **not executed** (would require
  interactive approval).
* **ZCode jump**: blocked by the app failing to stay running (evidence above).
* **Codex UI title ≠ stored title**: the app shows an AI-generated conversation
  title while the database stores the first prompt, so exact-title matching fails;
  L3 is verified through a real navigation (title change) and L2 falls back to
  app-level. Documented in `docs/JUMP.md`.
* **DSH AX route**: depends on Electron's AX tree exposing the sidebar rows
  (rendered items only) and requires Accessibility. A DSH update could change the
  tree; the route verifies the window title before claiming L3 and otherwise
  degrades, so it can never silently pretend to have switched sessions.
