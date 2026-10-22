# Jump verification record (real probes, 2026-10-21)

All results below were produced by running the **real release/debug binary** with
`--jump <task-id>` against the live apps on this Mac. Reproduce with
`scripts/jump-probe.sh`. An action is only reported as a level when a read-back
signal confirmed it; “command sent” is never counted.

## Implemented ladder

1. `TargetValidity::Invalid` → fail immediately (`workspace_missing`, `rollout_missing`, …) with next step `dismiss`. No blind app activation.
2. **L3** — session/tab:
   * host deep link (`open <url>`) + read back frontmost process and window titles;
     * exact title match (DSH-style hosts), or
     * a **new window title** compared to the pre-jump snapshot (Codex AI titles);
   * Terminal tab: `ps`+`lsof` resolves the controlling tty, AppleScript selects the
     tab by tty and reads it back.
3. **L2** — window: System Events AX window list → `perform action "AXRaise"` +
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
  support) so declared limitations (DSH L1, ZCode blocked) are visible in the
  app instead of only in this document.

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

* An L1-only adapter (DSH) reports **success** at its declared maximum level
  instead of a permanent "degraded" toast; degradation is now
  `requested > achieved`.
* When the L2/L3 attempts fail because of a missing privacy grant, the final
  outcome carries `permission_needed` + `next_step: "grant_permission"` even if
  the app itself was activated, so the toast offers the grant button instead of
  an unexplained "focus unconfirmed".
