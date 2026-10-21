# Acceptance results by phase

Host: macOS 27.0.1 (26A434), arm64, single display 1920×1080 (scale 1).
Build: `cargo build --release` (Rust 1.99.1, Tauri 2.11.2). Date: 2026-10-21.
Every command below was executed on this machine; unexecuted items are labelled
**未执行** with repro steps instead of being marked passed.

---

## Phase 0 — repository inspection and capability baseline

| Check | Result |
|---|---|
| Repo / stack identified | Tauri 2 + Rust + static JS (Hoemr/agent-monitor v0.3.1), legacy Claude-Code-only scanner |
| Build & run method | `cargo build --release`; binary `src-tauri/target/release/agent-monitor`; `.app` via `scripts/bundle-mac.sh` |
| Real data sources for all 4 objects | Verified by reading files/SQLite and printing live counts (see below) |
| Jump interfaces verified | Codex L3, DSH L1, VSCode L2, Terminal L1 (evidence in `docs/JUMP.md`) |
| Capability matrix | `docs/CAPABILITIES.md`, verified/待验证/blocked separated |
| Assumptions recorded | `docs/PHASE0.md` §7 + “corrections” at the end |

Shortest data flow documented in `docs/PHASE0.md` §4. Actual file list matches
the plan (no names were invented before reading the repo).

---

## Phase 1 — unified data, real status, parallel sessions

Command: `src-tauri/target/release/agent-monitor --dump-tasks`

Observed (live):

```
dedupe: first=40 second=40 stable=true
total 40   attention 0
claude-code cli      completed 7      codex  vscode completed 7
claude-code vscode   completed 3      dsh    desktop completed 17
dsh desktop running 3                 zcode  desktop completed 3
parallel DSH sessions: dsh-llm-cc-switch 疑似异常 /
                       Training-free DRE literature positioning /
                       agent-monitor 重构与桌宠扩展：
level spread: L1=23  L2=10  L3=7
```

| Requirement | Result |
|---|---|
| Four objects show real records | **Pass** — Codex 7, DSH 20, ZCode 3, Claude Code 10 (all from their local stores, no fixtures) |
| Same agent, parallel sessions not merged | **Pass** — 3 concurrent DSH sessions listed independently; multiple Claude sessions per project separate |
| Duplicate scans do not duplicate rows | **Pass** — `first=40 second=40 stable=true`; unit tests cover the render hash |
| Status from real signals only | **Pass** — mapping table in `docs/CAPABILITIES.md`; 46 unit tests; waiting states only from explicit evidence |
| Exit/crash does not stay “running” | **Pass (logic)** — Codex open turn + app gone → `Unknown`; turn history `failed/interrupted` → `Failed`; DSH open turn + app gone → `Unknown` |
| Original monitoring regression checklist | **Pass** — see below |

### Legacy regression checklist

| Item | Evidence |
|---|---|
| Claude session discovered from `~/.claude/projects/**/*.jsonl` | 10 Claude tasks in the live snapshot |
| Two parallel sessions not merged | Separate ids `claude-code:<uuid>`; 7+3 rows |
| Rescan does not duplicate | `stable=true` after two consecutive scans |
| `tool_use` → running, `end_turn` → completed | `parse_jsonl` mapping + unit test `legacy_status_mapping_is_explicit` |
| `file-history` 120 s long-thinking rule | Ported unchanged (`session.is_active && status=="done" && silence<120`) |
| `ide/*.lock` PID liveness | Ported; live run showed `claude-code vscode` host for 3 sessions |
| Tokens/model/message metadata | Rendered as `summary` (e.g. `OPUS · 12 msg · 34 tok`) |
| Subagent transcripts excluded | Ported (`fname.contains("subagent")`) |
| Stale sessions hidden | 6 h retention (24 h for IDE-matched), documented |
| Tray show/hide/refresh/quit | Rebuilt and exercised (show/hide/reset/refresh/quit menu items exist; show + reset verified) |
| Click-through | **Removed intentionally** (replaced by the pet interaction model) |
| Language switch en/zh | Frontend dictionaries + footer toggle; UI selftest checks both dictionaries |
| No prompt bodies persisted | Only titles/short summaries; no conversation text is stored by the app |

---

## Phase 2 — graded jump and permission feedback

All jump evidence is in `docs/JUMP.md` (commands + before/after titles).

| Requirement | Result |
|---|---|
| Session-level terminal combinations | **未执行** — no CLI agent installed in Terminal; code path implemented (`resolve_terminal_tty` + AppleScript by tty) |
| Window-level combinations | **Pass** — VSCode: window raised and verified (`success/L2`) |
| VSCode target focuses its window | **Pass** — evidence #5 in JUMP.md |
| Independent apps use verified level | **Pass** — ChatGPT L3, DSH L1, Terminal L1, ZCode blocked (declared L1, reported unconfirmed/degraded) |
| Shared window is not mis-declared | **Pass** — DSH capped at L1; multiple window matches → `window_ambiguous` degraded outcome |
| Closed target / permission / app missing feedback | **Pass (code + tests)** — `Invalid` target fails with `workspace_missing` etc. and a dismiss action; AppleScript permission errors map to `needs_accessibility`/`needs_automation` with a “Grant permission” toast (unit tested) |
| No infinite retries / repeated permission dialogs | **Pass** — one attempt per click, no loops; settings only opened by the user's toast action |

**Known caveat:** the packaged app's first-run TCC prompts were **未执行**
(interactive). Running jumps from the shell inherited the shell's existing
Automation/Accessibility grant.

---

## Phase 3 — desktop pet and task list

Window geometry was measured with `CGWindowListCopyWindowInfo` (no screen-recording
permission needed) on the release app:

| Mode | Expected | Observed |
|---|---|---|
| collapsed, right edge | 72×72 at right edge | `X=1848 Y=435 W=72 H=72` |
| expanded (40 tasks, 5 visible rows) | 360×390, right-anchored inward | `X=1560 Y=435 W=360 H=390` |
| edge-hidden | 14 px sliver visible at right | `X=1906 Y=435 W=72 H=72` → visible 14 px |
| recall via second launch (single-instance) | visible collapsed pet | `X=1848 Y=435 W=72 H=72` |
| focus at launch | current app keeps focus | frontmost stayed `访达` → `访达` |

UI acceptance is automated headlessly (`npm run ui-test`, Chrome drives real DOM
events in the shipped page): **ALL PASS, 24 checks**, including:

* rows = real task count, six fixture agents/statuses rendered with distinct glyphs;
* `aria-label` contains agent, title, status, level and action;
* two synchronous clicks → **one** jump;
* click immediately after a drag release → **no** jump, next click works;
* hover does not expand before 150 ms, expands after; panel becomes visible;
* `Escape` collapses;
* dismiss does not jump and removes the row;
* empty snapshot shows an explicit empty state and **zero** rows;
* **20 sessions** → panel stays 390 px high, list scrolls (1280 > 316), render < 1 ms;
* a status update while collapsed does **not** expand the pet;
* a degraded jump shows a non-blocking toast;
* both language dictionaries present.

Reliable recall: tray menu `Show Pet`, `Hide to Edge`, `Reset Position`, plus a
second launch (single-instance handler) restores a visible pet — verified.

---

## Phase 4 — displays, performance, extension, docs

| Item | Result |
|---|---|
| Two displays left/right + stacked, no overflow | **未执行** (single display). Geometry unit-tested for stacked/negative coordinates, screen removal, resolution change, Dock/menu-bar visible frame |
| Display unplug / resolution change | **未执行** physical; `ScaleFactorChanged` + periodic `reconcile_screens` implemented and compiled |
| Menu bar / Dock not covering main entry | **Pass** — all placement uses `Monitor::work_area()` (visible frame), unit tests + observed Y offsets (menu bar 30 px) |
| Space switch does not recreate window / lose entry | **Partial** — one window, `visibleOnAllWorkspaces=true`; actual Space/full-screen switching **未执行** |
| Performance (hidden / idle / active) | See `docs/PERF.md` (measured with `scripts/perf.sh`, release build) |
| New agent via simulation, no UI change | **Pass** — `AGENT_MONITOR_SIMULATED=1 --dump-tasks` returns 3 synthetic records (`SIM`, `origin=synthetic`); they render through the same snapshot path |
| Docs delivered | README, PHASE0, CAPABILITIES, JUMP, ACCEPTANCE, PERF, NEW_AGENT, UI_CONTRACT |

---

## Unexecuted items and reproduction steps

1. **Second physical display / hot-plug**
   Connect an external display (extend, not mirror), drag the pet across, unplug it:
   the pet must be clamped onto the remaining display. `scripts/perf.py` is
   irrelevant; use `--dump` plus `wins` probe. Geometry unit tests already cover the math.
2. **Space / full-screen / Stage Manager**
   Switch Spaces with the pet expanded and over a full-screen app; verify a single
   window remains discoverable (tray → Show Pet).
3. **TCC first-run prompts**
   `open "dist/Agent Monitor.app"`, click a task, confirm the Automation prompt text
   matches `NSAppleEventsUsageDescription`, deny once → toast shows
   `needs_automation` with “Grant permission”, no repeated system dialog until clicked again.
4. **Terminal tab L3**
   Install `claude` (or any CLI agent), run it in Terminal, click its task:
   expected AppleScript selects the tab by tty and reports `success/L3`.
5. **ZCode jump**
   Launch `ZCode.app` successfully (it currently exits immediately), then retest
   `zcode://workspace/open?path=`.
