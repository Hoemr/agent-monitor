# Agent Monitor — desktop pet for multi-agent coding sessions (macOS)

A small, quiet **menu-bar/tray-only** desktop pet (no Dock icon) that shows every
parallel coding-agent session in one place — newest activity first — and jumps to
the right window/session when you click a task.

It is built to look like a native macOS popover: real **vibrancy material**
(NSVisualEffectView `popover`), SF system typography, semantic system colours,
hairline separators, and an interface that follows the system light/dark
appearance. No webfonts, no dashboard chrome.

Built on the existing Agent Monitor project (Tauri 2 + Rust + vanilla JS) — the
monitoring core was refactored rather than replaced, the old Claude-Code-only
scanner was ported into a proper adapter, and the always-on widget became a
dockable, edge-snapping pet.

| Dark appearance | Light appearance |
|---|---|
| ![expanded dark](screenshots/pet-expanded-dark.png) | ![expanded light](screenshots/pet-expanded-light.png) |

*(Previews rendered from the shipped HTML/CSS with the labelled `SIM` fixture, over a desktop backdrop to show the translucent material.)*

### Menu-bar icon

![tray icons](screenshots/tray-icons.png)

A proper macOS **template image** (72 px source, rendered as an 18 pt status item, tinted
automatically for light/dark menu bars): a three-session mark that matches the pet, and a
variant with a notification dot when a task needs you. Left click shows/collapses the pet;
right click opens the menu (localized to the macOS system language).
| ![collapsed dark](screenshots/pet-collapsed-dark.png) | ![collapsed light](screenshots/pet-collapsed-light.png) |

## Supported agents (real local data, no APIs assumed)

| Agent | Source | Multi-session | Highest verified jump |
|---|---|---|---|
| **Codex** (ChatGPT app + VSCode extension) | `~/.codex/state_5.sqlite` + rollout JSONL + `thread_history_1.sqlite` | thread id | **L3** `codex://threads/<id>` (verified) |
| **DSH** (DeepSeek Harness) | `~/.dsh/storages/session_projcache/sessions/*.json` | session dir | **L1** (host has no session URL — documented) |
| **ZCode** | `~/.zcode/v2/tasks-index.sqlite` | `task_id` | **blocked** on this host (`ZCode.app` exits immediately); data/mapping implemented |
| **Claude Code** (`anthropic.claude-code` **VSCode extension**) | `~/.claude/projects/**/*.jsonl`, `file-history/`, `ide/*.lock` | transcript UUID | **L2** VSCode window (verified) |

Full matrix with evidence and limitations: [`docs/CAPABILITIES.md`](docs/CAPABILITIES.md) ·
jump probes: [`docs/JUMP.md`](docs/JUMP.md) · acceptance: [`docs/ACCEPTANCE.md`](docs/ACCEPTANCE.md).

## Features

* One unified task list ordered by **most recent activity first** (running
  conversations naturally rise to the top; waiting/failed tasks are highlighted
  with a badge and accent, never by re-grouping the list): agent, title, unified
  status (running / waiting input / waiting approval / completed / failed / idle /
  unknown), real last-activity time, short summary, and the actual jump level
  (`L3`…`L0`).
* **Jump with honest degradation**: session → window → app → nothing. A level is
  reported only after a read-back check; failures/degradations show a short toast
  with a “Grant permission” action when needed.
* **Desktop pet**: drag anywhere, snap to the nearest screen edge, hide to a 14 px
  sliver, hover/click to expand inward, 5 visible rows with scroll, quiet idle dot.
* **Multi-display aware**: placement uses each screen's *visible frame* (menu bar +
  Dock excluded), restores by screen identity + relative position, and recovers
  when a screen disappears.
* **Tray-only**: accessory activation policy — no Dock icon, no app-switcher
  entry; everything lives in the menu-bar item. Reliable recall: tray → Show Pet /
  Reset Position; a second launch also restores the pet. It never steals keyboard
  focus on launch, updates, or hover.
* **Native macOS look**: translucent popover material that follows system
  light/dark appearance; SF Pro / PingFang SC typography; one state dot per row
  plus a text label (never colour alone); title-first hierarchy; no uppercase
  labels, no card grids, no decorative gradients.
* **Low resource use**: event-driven file watching with a slow fallback tick; no
  per-task timers; no faster-than-30 s UI timer; identical snapshots are not
  re-rendered.
* **Local only**: reads local files/SQLite read-only. No upload, no telemetry, no
  network calls. Titles stay short; conversation bodies are not stored.

## Build and run

Requirements: macOS 14.5+ (for `light-dark()` appearance support), Rust (stable), Command Line Tools. No Xcode project and
no Node build step are required.

```bash
# production build (frontend assets are embedded via tauri's custom-protocol)
cargo build --release --manifest-path src-tauri/Cargo.toml
src-tauri/target/release/agent-monitor          # run the pet

# verify adapters without opening a window
src-tauri/target/release/agent-monitor --dump-agents
src-tauri/target/release/agent-monitor --dump-tasks

# one real jump (prints the graded outcome)
src-tauri/target/release/agent-monitor --jump 'codex:<thread-id>'
```

### macOS .app bundle

```bash
bash scripts/bundle-mac.sh     # → dist/Agent Monitor.app (ad-hoc signed, LSUIElement)
open "dist/Agent Monitor.app"
```

The first time you click a task, macOS may ask for **Automation** (to focus the
target window) and **Accessibility** (to raise a specific window). The app only
uses them on your click, explains the need in the toast, and links to the exact
System Settings pane. It never resets or forces permissions.

## Development

```bash
npm test          # cargo test (46 unit tests)
npm run ui-test   # 24 headless UI checks (Chrome drives the real page)
npm run perf      # 5 min × hidden / idle / expanded sampling of the release app
npm run jump-probe
```

Layout/design notes for the frontend contract: [`docs/UI_CONTRACT.md`](docs/UI_CONTRACT.md).
Adding an agent: [`docs/NEW_AGENT.md`](docs/NEW_AGENT.md) — one adapter file + one
registry line; no UI or monitor changes.

Extension acceptance uses the clearly-labelled synthetic adapter:

```bash
AGENT_MONITOR_SIMULATED=1 src-tauri/target/release/agent-monitor --dump-tasks
```

## Known limitations

* Codex shows an AI-generated window title while its database stores the first
  prompt, so Codex uses navigation evidence for L3 and cannot do exact-title L2.
* DSH is a single-window Electron app whose only deep link is `dsh://open` (focus),
  so session-level jumping is impossible and is reported as L1.
* `ZCode.app` (Electron 41) exits immediately on the test host (macOS 27 beta);
  its data is shown, its jump is reported as unconfirmed. Details in `docs/JUMP.md`.
* Claude Code is treated as the **VSCode extension host** only (per project scope);
  terminal-hosted Claude sessions are not routed to Terminal.
* Physical multi-display, Space/full-screen, and first-run TCC prompts were **not
  executed** on this single-display host; see `docs/ACCEPTANCE.md` for repro steps.
* Performance numbers are measured on the test Mac in `docs/PERF.md` — budget
  figures there are observations, not platform guarantees.
