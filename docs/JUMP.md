# Jump verification record (real probes, 2026-10-21)

All results below were produced by running the **real release/debug binary** with
`--jump <task-id>` against the live apps on this Mac. Reproduce with
`scripts/jump-probe.sh`. An action is only reported as a level when a read-back
signal confirmed it; “command sent” is never counted.

## Implemented ladder

1. `TargetValidity::Invalid` → fail immediately (`workspace_missing`, `rollout_missing`, …) with next step `dismiss`. No blind app activation.
2. **L3** — session/tab:
   * host deep link (`open <url>`) + read back frontmost process and window titles;
     * exact title match, or
     * a **new window title** compared to the pre-jump snapshot (Codex AI titles);
   * **DSH sidebar session click** (`host_ref = "dsh-sidebar"`, round 2): native
     macOS AX read of the sidebar rows → match on session title + project (cwd
     basename) + relative time → synthetic click on the row → focused window title
     read-back (`<session> — DeepSeek Harness`). Requires Accessibility; DSH has no
     session URL, so this is the only possible session route;
   * Terminal tab: `ps`+`lsof` resolves the controlling tty, AppleScript selects the
     tab by tty and reads it back.
3. **L2** — window:
   * **VSCode project-precise** (round 2): `code --reuse-window <workspace_path>`
     focuses the already-open window for that project even with many VSCode
     windows; verified by frontmost `Code` + AX focused window title containing
     the workspace basename (Fallback: `open -b com.microsoft.VSCode <path>`);
   * otherwise System Events AX window list → `perform action "AXRaise"` +
     `set index of window to 1` → read back frontmost process. Multiple matching
     windows are focused but reported as `window_ambiguous` (degraded) rather than
     claiming a specific session.
4. **L1** — `open -b <bundle>` (fallback `open -a <name>`) → frontmost verification;
   if it cannot be confirmed the outcome is `unconfirmed`.
5. **L0** — no target → `no_target`/`task_gone`.

Permissions: AppleScript errors (`-1719/-1743/-25211`, “not allowed assistive
access”, “not authorized”) map to `needs_accessibility` / `needs_automation`,
returned with `permission_needed` + a “Grant permission” toast action. The app
never opens System Settings by itself and never resets TCC.

## Measured results

| # | Object / host | Action | Evidence (before → after) | Outcome |
|---|---|---|---|---|
| 1 | Codex / ChatGPT app | `codex://threads/01a15a27-a527-7301-869e-6c88b1432ca2` | ChatGPT window title `编写 Agent 桌宠实现 Prompt` → `调研密度比估计与扩散生成`; frontmost = ChatGPT | **success / L3** (`opened_session`) |
| 2 | Codex / ChatGPT app (before verification fix) | same deep link | App came forward but verification could not prove the session because the UI title is AI-generated and differs from the stored title | **degraded / L1** — the code was then fixed to verify by title change (see #1) |
| 3 | DSH / DeepSeek Harness | `dsh://open/<session-id>` attempt | Window title unchanged (`Training-free DRE literature positioning audit`); main-process source shows only `dsh://open` / `dsh://open/` → `focusPrimaryWindow()` | **degraded / L1** (`activated_app_only`) — no session route exists |
| 4 | DSH / DeepSeek Harness | `open -b com.deepseek.dsh` | Frontmost = DeepSeek Harness | **L1 works**, declared max = L1 |
| 5 | Claude Code / VSCode | L2 raise window for `~/Paper/DropDPO-Confidence` | Window 1 `arr_submission.tex — DropDPO-Confidence` → `results.md — DropDPO-Confidence`; frontmost = Code | **success / L2** (`opened_window`) |
| 6 | ZCode | `open -b dev.zcode.app`, `open -a ZCode`, direct binary | `open` exits 0, no process, no window, no crash report; Electron 41 | **blocked** — cannot verify any level on this host |
| 7 | Claude Code CLI / Terminal | L3 tab selection by tty | No `claude` CLI installed; project scope now treats Claude Code as VSCode-hosted, so this path is retained but unused | **未执行 / not routed** |

## Notes that changed the design

* **Codex window titles are AI-generated.** `threads.title` stores the first user
  prompt (often a long paragraph) while the window shows a short generated title.
  Exact-title L2/L3 matching therefore cannot use the stored title; L3 is verified
  by observing a real navigation, and L2 for Codex effectively degrades to the app
  unless a title hint is available.
* **DSH has one window for all sessions.** Its Electron `open-url` handler only
  recognises `dsh://open`; there is no per-session URL. Declaring L3 would be a
  false promise, so DSH is capped at L1 and the UI explains the limitation.
* **Claude Code = VSCode extension.** Per project clarification, Claude Code
  records are modelled as VSCode-hosted sessions (`host = "vscode"`, target =
  VSCode window by workspace). The terminal tab resolver remains in `jump.rs` but
  no Claude task is routed to Terminal.
* **ZCode cannot be verified on this host** because the app exits immediately.
  Its index data is still displayed with real statuses; jump degrades/reports
  unconfirmed instead of pretending.
* Terminal tab selection is implemented (`resolve_terminal_tty` + AppleScript by
  tty) but cannot be verified until a CLI agent is installed and running in
  Terminal.

## Repro

```bash
# build once
source ~/.cargo/env && (cd src-tauri && cargo build --release)
BIN=src-tauri/target/release/agent-monitor

# list real tasks + jump targets
$BIN --dump-tasks | jq '.tasks[] | {id,title,level,target}'

# Codex: jump to a real thread, verify window title changes
osascript -e 'tell application "System Events" to tell process "ChatGPT" to get title of every window'
$BIN --jump 'codex:<thread-id>'
osascript -e 'tell application "System Events" to tell process "ChatGPT" to get title of every window'

# DSH: app-level only (expected degraded)
$BIN --jump 'dsh:<session-id>'

# VSCode window-level
$BIN --jump 'claude-code:<session-uuid>'
osascript -e 'tell application "System Events" to tell process "Code" to get title of window 1'

# ZCode launch probe (documents the blocker)
open -b dev.zcode.app; sleep 3; pgrep -fl 'ZCode.app/Contents/MacOS/ZCode' || echo "no ZCode process"
```

## Setup, permissions and self-check (Settings → Jump)

Jumping needs two macOS privacy grants for **Agent Monitor.app**; without them
the app can still show sessions, but it cannot locate/raise the right window:

| Grant | System Settings pane | Needed for |
|---|---|---|
| **Accessibility** | Privacy & Security → Accessibility | reading window titles / raising a window (`AXRaise`) |
| **Automation** (→ System Events) | Privacy & Security → Automation | the `osascript` calls that drive System Events |

The packaged app is **ad-hoc signed** (`codesign -dv` shows `Signature=adhoc`).
macOS keys TCC grants to the code signature, so after rebuilding/replacing
`Agent Monitor.app` the grants may be forgotten — re-add the new copy once. This
is the most common reason "jump suddenly stopped working".

### Settings → Jump now contains a guided setup

* status pills for Accessibility / Automation / window access, refreshed by a
  **Check again** button;
* buttons that open the exact System Settings pane (`open_permission_settings`)
  and reveal the app bundle in Finder (`reveal_app_in_finder`) so it can be
  dragged into the Accessibility list;
* a three-step first-run guide and a note about ad-hoc re-authorisation;
* the per-agent capability table (max level, installed/running, deep-link
  support — Codex/DSH session routes, VSCode project windows, ZCode blocked)
  so declared limitations are visible in the app instead of only in this document.

### Command `get_jump_diagnostics`

```jsonc
{
  "accessibility": { "granted": false, "detail": null },
  "automation":    { "granted": true,  "detail": null },
  "window_access": { "granted": false, "detail": "needs_accessibility" },
  "system_events": true,
  "app_bundle_path": "/Applications/Agent Monitor.app",
  "agents": [{
    "key": "codex", "name": "Codex",
    "bundle_id": "com.openai.codex", "app_name": "ChatGPT",
    "max_level": "L3", "notes": "codex://threads/<id> route; verified …",
    "installed": true, "running": true, "deep_link": true
  }]
}
```

* `accessibility` uses `AXIsProcessTrusted()` (ApplicationServices, no extra crate);
* `automation` runs a harmless System Events process query and classifies the
  error (`needs_accessibility` / `needs_automation` / `osascript_failed`);
* `window_access` is the real read test (window titles of a running agent app);
  it is what L2/L3 actually need;
* `installed` uses LaunchServices (`mdfind` by bundle id) with `/Applications`
  fallbacks; `running` matches the app's process path from `ps`;
* `deep_link` is the adapter capability, not a promise that the host supports it.

`--jump-diagnostics` prints the same JSON headlessly for support tickets.

### Jump outcome fixes (same release)

* An app-level-only jump reports **success** when it reaches its declared
  maximum level (this was the DSH L1 fix in 0.5.0; DSH reaches L3 since round 2)
  instead of a permanent "degraded" toast; degradation is now
  `requested > achieved`.
* When the L2/L3 attempts fail because of a missing privacy grant, the final
  outcome carries `permission_needed` + `next_step: "grant_permission"` even if
  the app itself was activated, so the toast offers the grant button instead of
  an unexplained "focus unconfirmed".


## Round 2 — project/session-precise jump (0.6.0, measured 2026-10-22)

The earlier "DSH capped at L1" finding is superseded: the host still has **no
session URL** (`dsh://open` only focuses the app), but the sidebar is exposed
through the native macOS AX API, and a real click on a session row switches the
window. This is implemented with a small AX/CoreGraphics bridge
(`src-tauri/src/ax.rs`, no new crates): read `AXRow`s of the sidebar outline,
score them, click the matched row's frame with `CGEvent`, read back the focused
window title.

### Matching evidence (never a guessed click)

A DSH session row is scored on three independent signals:

1. **title** — normalized exact match, then bidirectional prefix (≥6 chars) so
   truncated sidebar labels still match; leading `#`/trailing ellipsis normalized;
2. **project** — `target.workspace_path` basename vs the sidebar workspace row
   (DSH groups sessions by project, which disambiguates duplicate titles);
3. **relative time** — the row's `刚刚/N分钟/N小时/N天` label vs
   `now - last_activity_secs`, as a tie-breaker.

If two different rows tie, the outcome is `session_ambiguous` and **no click is
made**. After activation the sidebar re-lays out (transient 1 px frames / partial
rows), so the route waits for a sane frame that is identical across two
consecutive AX reads, clicks immediately, and on a verify miss re-reads and
retries once; only then `session_click_unverified`.

### Measured results (real app, real DSH data)

| Action | Evidence (before → after) | Outcome |
|---|---|---|
| `--jump dsh:session-0bfd41d5…` ("# 给 agent-monitor 增加“设置页 +") | focused title `多窗口询问提醒与跳转 — DeepSeek Harness` → `# 给 agent-monitor 增加“设置页 + — DeepSeek Harness` | **success / L3 `opened_session`** (3/3 round-trips; row stable after 2 reads) |
| jump back to `dsh:session-9ab2d0aa…` (this conversation) | window title returns to `多窗口询问提醒与跳转 — DeepSeek Harness` | **success / L3** |
| already visible session | no click; focused title read-back matches | **success / L3** |
| subagent session not rendered in the root sidebar | no click; app activated only | **degraded / L1 `session_row_not_found`** (honest; window unchanged) |
| `--ax-rows <pid>` | trusted=true, 14–20 sidebar rows with frames + focused title | read-only verification aid |

### VSCode project-precise window jump (real, two windows)

Setup: two VSCode windows open (`Welcome — AgentMonitor` focused,
`arr_submission.pdf — DropDPO-Confidence` in the background). Jump to a Claude
Code session whose workspace is `~/Paper/DropDPO-Confidence`:

```text
focused before: Welcome — AgentMonitor
--jump claude-code:625572ae… → success / L2 / opened_window
detail: arr_submission.pdf — DropDPO-Confidence
focused after : arr_submission.pdf — DropDPO-Confidence   (window order swapped)
```

`code --reuse-window <path>` was resolved from `/opt/homebrew/bin/code`; without
the CLI the route falls back to `open -b com.microsoft.VSCode <path>` and then to
the legacy AX raise, so nothing regresses.

### New reason keys (pet toast i18n)

`ax_unavailable`, `session_row_not_found`, `session_ambiguous`,
`session_click_unverified` (plus existing `needs_accessibility`). All are
reported with an actionable Chinese/English message; failures never leave the UI
silent.

### Verification footer count (same release)

The expanded popup now shows `visible / total sessions` (e.g. `5 / 119 个会话`)
instead of the raw total, matching the rows the window can display.


## Round 3 — workspace auto-expand + verification polling (0.7.0, measured 2026-10-22)

Two fixes for the "warning after a successful jump" and "DSH warns every time"
reports:

1. **DSH collapses non-active projects.** Only the active workspace renders its
   session rows; a session in any other project was `session_row_not_found`, so
   clicking it activated DSH and warned. The route now expands the target
   project first: after activation and a failed selection it finds the workspace
   row by `target.workspace_path` basename (normalised exact, then a unique
   contains; ambiguity ⇒ no click), clicks it, polls up to 1.5 s for the session
   row, then runs the usual stable select/click/verify. A genuinely missing
   project reports the new `project_not_found`; an expanded-but-absent session
   (e.g. subagent-only) still reports `session_row_not_found`.
2. **Activation/title verification races.** Fixed sleeps + one read classified
   slow-but-successful jumps as `focus_unconfirmed` / `deep_link_unverified` /
   `session_click_unverified` (and therefore warned). Verification now polls
   every 120 ms for up to 2 s (deep-link navigation 1.2 s; DSH title 1.5 s) in
   all L1/L2/L3 paths and only reports a failure after the real timeout. Codex
   skips the useless AI-title L2 probe after a deep-link attempt, cutting a
   stale-thread failure from 7.1 s to 4.6 s.

Measured on the live host (real release build):

| Jump | Outcome | Time |
|---|---|---|
| DSH session in collapsed project 审稿 (`dsh:session-0c664f1d…`) | **success / L3**, logs `expanding workspace "审稿"`, detail `论文核心主张简要概述 — DeepSeek Harness` | 2.4 s |
| DSH session in the active project / already visible | success / L3, no click | < 1 s |
| Codex live thread (deep link navigates) | success / L3 | 0.6 s |
| Codex deleted/old thread (app only comes forward) | degraded / L1 `deep_link_unverified` (one honest notice; the pet dedupes repeats) | 4.6 s |
| VSCode project window (`code --reuse-window`) | success / L2 | 1.5 s |

The pet UI additionally shows soft (`degraded`/`unconfirmed`) jump notices **once
per task+reason per session**, so repeated clicks on the same stale target stay
silent; hard failures are still always reported.


## Permission repair — "checked in System Settings but still Not granted" (0.7.1)

macOS TCC binds Accessibility/Automation grants to the code identity of the
bundle. This app is **ad-hoc signed**, so every rebuild produces a new cdhash:
System Settings can keep showing an old, still-checked Agent Monitor entry while
the currently running copy is not actually trusted. Running a different copy
(e.g. `dist/Agent Monitor.app` vs an installed `/Applications/…`) has the same
effect. The Settings window also used to check once, so it could simply be
**stale UI** after the user toggled the switch.

Settings → Jump now handles both cases:

1. **Auto refresh**: regaining focus (or becoming visible) while the Jump pane is
   selected re-runs `get_jump_diagnostics`; the pills update after returning
   from System Settings. No polling.
2. **App location** row: shows `app_bundle_path` of the running bundle, so the
   user can confirm the entry in System Settings is that exact copy.
3. **Request permission** button → `request_accessibility_permission()`: calls
   `AXIsProcessTrustedWithOptions(AXTrustedCheckOptionPrompt=true)` in the app's
   own process, so macOS offers to add the current binary to Accessibility.
4. **Reveal App** → `reveal_app_in_finder()` to drag the current bundle into the
   list (or use the `+` button).
5. **Reset this app's permissions…** → `reset_app_permissions()`: inline-confirmed
   `tccutil reset Accessibility com.agent.monitor` and `tccutil reset AppleEvents
   com.agent.monitor`, then re-add the current build. The argv builder is
   hard-guarded and unit-tested to always include the bundle id; it never resets
   other apps.
6. The first-run guide and the ad-hoc re-sign explanation are collapsed inside a
   `<details>` element, so the pane stays compact. A one-line amber alert appears
   only when Accessibility is missing; the same repair copy is inside the guide.

Debug/acceptance aids (never set by normal users):

```bash
# Write the current jump diagnostics JSON ~1.5 s after startup (works in the real .app)
launchctl setenv AGENT_MONITOR_DUMP_JUMP_DIAGNOSTICS /tmp/am-jump-diag.json
open -n "dist/Agent Monitor.app"; sleep 3
cat /tmp/am-jump-diag.json
launchctl unsetenv AGENT_MONITOR_DUMP_JUMP_DIAGNOSTICS

# Ask macOS for the Accessibility prompt for the current app ~2 s after startup
launchctl setenv AGENT_MONITOR_REQUEST_AX_PROMPT 1
open -n "dist/Agent Monitor.app"
launchctl unsetenv AGENT_MONITOR_REQUEST_AX_PROMPT
```

> **Do not run `tccutil reset Accessibility` without a bundle id.** It clears the
> machine-wide Accessibility records of *every* app on the Mac, and those grants
> can only be restored by the user in System Settings → Privacy & Security →
> Accessibility. The app's own reset command is restricted to
> `com.agent.monitor` and refuses to run without a bundle id.

### Measured in-app diagnostics (real packaged app, round 5)

The debug dump launched against `dist/Agent Monitor.app` reports:

```jsonc
{
  "accessibility": { "granted": false, "detail": null },
  "automation":    { "granted": true,  "detail": null },
  "window_access": { "granted": false, "detail": "needs_accessibility" },
  "system_events": true,
  "app_bundle_path": "…/dist/Agent Monitor.app"
}
```

The `false` here is the true state of the host after an accidental machine-wide
Accessibility reset during testing (see `docs/ACCEPTANCE.md` Phase 9); it proves
the dump reads the **real bundle** and reports honestly. Once the user re-grants
Accessibility for this copy, the same dump/pane shows `true` and window-level
jumps work again.
