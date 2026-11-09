# Capability matrix (verified on this Mac)

Environment: macOS 27.0.1 (26A434), arm64, single display 1920×1080 @ scale 1,
Rust 1.99.1, release build (`cargo build --release`, Tauri 2.11 + custom-protocol +
macOSPrivateApi). Initial audit: 2026-10-21; VSCode routes updated 2026-10-28.

Legend: **L3** session/tab, **L2** window, **L1** application, **L0** unavailable.
“Verified” means a real action was executed and the result was read back
(see `docs/JUMP.md` for commands and observed evidence). Declared capability is
never higher than the verified level.

2026-10-30 CLI collection: the complete Codex home list is read from
`codex-homes.json` in the app data directory (default: `~/.codex`). This host's
list includes the `codex-home` used by `打开Codex副账号CLI.command`.
Interactive CLI records are labelled Terminal and target Terminal at L1.
The desktop and VSCode routes below are unchanged.

Codex desktop read synchronization (2026-11-03): the read-only
`thread_read_receipts` table is checked against the exact completed rollout turn.
An empty `first_unread_turn` (SQL NULL) and matching `last_published_turn` clear
that completion's green marker, including conversations viewed before the monitor
sampled the foreground. Missing/older receipts and terminal/VSCode sessions are
not acknowledged by this path. Native foreground matching remains the fallback.

Codex CLI read synchronization: CLI sessions do not publish the desktop read
receipts. For a foreground Terminal.app tab, a TTY query plus attached processes'
open rollout paths identifies the active conversation, including sessions in a
secondary CODEX_HOME. Matching is independent of title and working directory.
Only its exact completed version is acknowledged; running/ambiguous sessions
and other tabs remain unread. The native Terminal Automation grant is required.

Antigravity desktop reads `~/.gemini/antigravity`, and CLI reads
`~/.gemini/antigravity-cli`. Both indices and per-conversation databases are
opened read-only with live WAL support. Empty launch/login sessions and child
agents are excluded; desktop/CLI IDs remain distinct even for the same UUID.
Completion requires either a DONE FINISH step or an idle parent with a DONE
planner response, `STOP_REASON_STOP_PATTERN` and no tool calls. Ordinary text,
completed tools, token limits and an idle index alone never prove completion.
Planner response field numbers and enums were checked against this Mac's
installed desktop `language_server` descriptors on 2026-11-03. Three real desktop
conversations contain final planner responses. WAITING steps require explicit
ask-question/permission requests for waiting states (covered by fixtures; not yet
observed live). Model names come from the current parent planner step's
`SourceTrajectoryStepInfo` and its matching `gen_metadata.chat_model` record
(`model_display_name`, then actual response model IDs), or the recorded `ModelInfo`.
The latest user-input step bounds the lookup, so a new turn cannot inherit an old
model. Model/DB WAL stamps are cached, and numeric placeholder enums are never
presented as real names. Four real desktop sessions verified on 2026-11-04 include
Gemini 4 Argon (Medium) and Claude Opus 5.5 (High). Desktop clicks target
Antigravity at L1; CLI clicks target Terminal. Exact-session navigation is unverified.

| Object | Real data source (read-only) | Multi-session identity | Highest verified jump | Required permission | Degradation behaviour | Test result |
|---|---|---|---|---|---|---|
| **Codex** (ChatGPT app + VSCode extension `openai.chatgpt`) | `~/.codex/state_5.sqlite` `threads` (192 rows) + rollout JSONL tail + `thread_history_1.sqlite` `thread_turns` | `threads.id` (UUID); 61 threads were created in the VSCode extension, rest in the app | **L3 — verified**: desktop `codex://threads/<id>`; VSCode `vscode://openai.chatgpt/local/<id>` after opening the workspace, verified through the focused chat. Desktop changes the visible thread (window title changed from `编写 Agent 桌宠实现 Prompt` → `调研密度比估计与扩散生成`, outcome `success/L3`) | Automation (used only to read back the window title); Accessibility for AXRaise fallback | Deep link unverified or app not running → L2 window match → L1 activates the identified desktop or VSCode host → toast; only the app is activated when the session cannot be proven | **Pass** (real record + real L3) |
| **DSH** (DeepSeek Harness desktop) | `~/.dsh/storages/session_projcache/sessions/*.json` (67 sessions: title, cwd, `turnBoundary`, `userQuestions`, stats) | Session directory name (`session-<uuid>`); sidebar groups sessions by workspace; 3 parallel sessions were visible at the same time in the live run | **L3 — verified (round 2)**: the host has no session URL (`dsh://open` only focuses the app), but the sidebar is exposed through the native macOS AX API; matching the row on session title + project (cwd basename) + relative time and clicking it (CGEvent) switches the window — verified by the focused window title `<session> — DeepSeek Harness` (switched away and back on 2026-10-22). | Accessibility (AX tree + synthetic click); Automation not needed for this route | Sidebar row not rendered (virtualized list) → app focus with a precise reason; duplicate titles → project/time evidence, otherwise `session_ambiguous` instead of a wrong click; already-visible session → success without clicking | **Pass** (real record + real L3 with read-back) |
| **ZCode** (desktop / macOS) | Live read-only tasks index + transcript evidence | `tasks.task_id` (`sess_<uuid>`) | **L3 — verified**: current indexed ID/title → native task search → exact result AX action → active H1 read-back. Same-workspace A/B and cross-workspace EEG verified on 2026-11-08 | Accessibility | Duplicate/unnamed titles or uncertain UI state → precise failure/unconfirmed result; no workspace navigation as a substitute. Overlapping switches are rejected | **Pass** for uniquely named indexed sessions |
| **VSCode agent plugin** — Claude Code 2.1.311 | Local JSONL parent-turn events | Transcript UUID; parallel sessions stay separate | **L3**: open project and extension session URI; verify the focused chat ID or unique source title | Accessibility + Automation | Unconfirmed chat → L2 project window; VSCode may require extension-URI confirmation | **Pass**: chat title and transcript UUID verified on 2026-10-28 |
| **Antigravity desktop + CLI** | Local SQLite summaries and step protobufs in the two directories above | `antigravity:desktop:<uuid>` / existing `antigravity:<uuid>` | L1 application route declared; exact-session route unverified | Automation for activation confirmation | Opens the corresponding host application | Three real desktop final results read; status transitions, WAL and completion identities tested with fixtures |
| **Simulated Agent** (extension acceptance only, `AGENT_MONITOR_SIMULATED=1`) | In-process fixture, `origin = synthetic`, UI shows `SIM` | 3 fixture sessions | L1 fixture | none | Clearly labelled, never mixed with real adapters | **Pass** (see ACCEPTANCE §Phase 4) |

## Status mapping and evidence

| Unified status | Codex | DSH | ZCode | Claude Code | Simulated |
|---|---|---|---|---|---|
| Running | rollout latest task lifecycle event is `task_started` (app alive, <30 m silence); tool/item/message completions do not end the task | `turnBoundary.openTurnStartSeq != null` (+ `lastStepBoundary.kind`), app alive | live sessions keep `task_status` **NULL/empty** and ZCode touches the row on every turn; empty status + `updated_at` fresh (≤5 min) = Running (**observed live**); `running/in_progress` literals never seen | `tool_use` / `thinking` / `responding` in transcript, with activity within 30 min and a live host | fixture |
| Waiting input | unanswered `request_user_input` / async question call in the current open turn | `userQuestions.questions.active[]` non-empty (**explicit**) | not detectable from the index alone: a question-blocked turn has the same NULL status and stops touching `updated_at`, so after 5 min it degrades to Unknown (no `waiting_input` literal observed) | unanswered parent `AskUserQuestion`; unrelated tool results cannot settle it | fixture |
| Waiting approval | not available | not available (policy `never`, no request event observed) | `waiting_approval` literal (**not observed**) | not available | fixture |
| Completed | latest task lifecycle event is `task_complete`; `thread_turns.status = completed` is used only when no rollout lifecycle event is available | explicit `openTurnStartSeq: null` plus `lastStepBoundary.kind = end` (a missing field does not mean closed) | `completed` (**observed**; stamped only when the task ends) | parent assistant `stop_reason = end_turn`, no tool calls; sidechain messages and silence never complete | fixture |
| Failed | latest task lifecycle event is `turn_aborted`, or `thread_turns.status ∈ {failed, interrupted}` when no rollout lifecycle event is available | — | `failed`, `cancelled` (**not observed**) | — | fixture |
| Idle | thread with no rollout/turns | session with no turns | `idle` (**not observed**) | — | — |
| Unknown | no state event / app died with open turn / >30 m silence | open turn but app not running / >30 m silence / no boundary state | empty status with no activity for >5 min (crashed and idle sessions are indistinguishable in the index) / unmapped status | open parent turn whose host exited, or with no transcript activity for >30 min / unmapped legacy status | — |

Every adapter must supply explicit final completion evidence; Monitor demotes
unsupported completion claims to Unknown. See `docs/NEW_AGENT.md`.

Rules enforced by tests: waiting states are only produced from explicit
signals; “no output” never becomes waiting; completed + closed window is shown as
completed **plus** an invalid target, not as failed; a reused session can return
to running.

## Ordering and presentation

Pinned tasks precede unpinned tasks. Each group sorts by **`last_activity_secs` descending** (newest first);
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
* **ZCode jump**: requires a uniquely named session in the live index and the current native task-search controls. Duplicate and unnamed sessions are refused safely.
* **Codex UI title ≠ stored title**: the app shows an AI-generated conversation
  title while the database stores the first prompt, so exact-title matching fails;
  L3 is verified through a real navigation (title change) and L2 falls back to
  app-level. Documented in `docs/JUMP.md`.
* **DSH AX route**: depends on Electron's AX tree exposing the sidebar rows
  (rendered items only) and requires Accessibility. A DSH update could change the
  tree; the route verifies the window title before claiming L3 and otherwise
  degrades, so it can never silently pretend to have switched sessions.
