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
| 5 | Claude Code / VSCode | L2 raise window for `/Users/wei/Paper/DropDPO-Confidence` | Window 1 `arr_submission.tex — DropDPO-Confidence` → `results.md — DropDPO-Confidence`; frontmost = Code | **success / L2** (`opened_window`) |
| 6 | Claude Code CLI / Terminal | L2/L1 | No window hint + no live claude process → `open -b com.apple.Terminal`; frontmost `-zsh` Terminal window appeared | **degraded / L1** (`no_window_hint`) |
| 7 | ZCode | `open -b dev.zcode.app`, `open -a ZCode`, direct binary | `open` exits 0, no process, no window, no crash report; Electron 41 | **blocked** — cannot verify any level on this host |
| 8 | Claude Code CLI / Terminal | L3 tab selection by tty | No `claude` CLI installed on this host | **未执行** (code path implemented; see repro) |

## Notes that changed the design

* **Codex window titles are AI-generated.** `threads.title` stores the first user
  prompt (often a long paragraph) while the window shows a short generated title.
  Exact-title L2/L3 matching therefore cannot use the stored title; L3 is verified
  by observing a real navigation, and L2 for Codex effectively degrades to the app
  unless a title hint is available.
* **DSH has one window for all sessions.** Its Electron `open-url` handler only
  recognises `dsh://open`; there is no per-session URL. Declaring L3 would be a
  false promise, so DSH is capped at L1 and the UI explains the limitation.
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
