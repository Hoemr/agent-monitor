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
| Jump interfaces verified | Codex L3, DSH L1 *(L3 since Phase 7)*, VSCode L2, Terminal L1 (evidence in `docs/JUMP.md`) |
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
| Task ordering: newest activity first | **Pass** — `last_activity_secs` strictly descending (`sorted = True`); status only breaks ties. Top rows in the live snapshot were the 3 running DSH conversations |
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
| Tray show/hide/refresh/quit | Rebuilt and exercised (show/hide/reset/refresh/quit menu items exist; show + reset verified); tray is the **only** entry point (Dock icon hidden) |
| Click-through | **Removed intentionally** (replaced by the pet interaction model) |
| Language switch en/zh | Frontend dictionaries + footer toggle; UI selftest checks both dictionaries |
| No prompt bodies persisted | Only titles/short summaries; no conversation text is stored by the app |

---

## Phase 2 — graded jump and permission feedback

All jump evidence is in `docs/JUMP.md` (commands + before/after titles).

| Requirement | Result |
|---|---|
| VSCode agent plugin is VSCode-hosted | **Pass** — Claude Code records now always use `host="vscode"` and target `com.microsoft.VSCode`; verified in `--dump-tasks` (`claude hosts: [('vscode', 'com.microsoft.VSCode')]`). No Claude task is routed to Terminal |
| Window-level combinations | **Pass** — VSCode: window raised and verified (`success/L2`) |
| VSCode target focuses its window | **Pass** — evidence #5 in JUMP.md |
| Independent apps use verified level | **Pass** — ChatGPT L3, DSH L1 *(L3 since Phase 7)*, ZCode blocked (declared L1, reported unconfirmed/degraded) |
| Shared window is not mis-declared | **Pass** — DSH capped at L1 at the time *(L3 via AX sidebar since Phase 7)*; multiple window matches → `window_ambiguous` degraded outcome |
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
| geometry after vibrancy change (screen was locked, `CGWindowList` unreliable) | internal window targets | `AGENT_MONITOR_DEBUG_GEOMETRY=1`: expanded target `Rect { x: 1560, y: 453.6, w: 360, h: 390 }`; edge-hidden target `x: 1906, w: 72` (14 px visible); collapsed start `72×72`; work-area picked up the lock-screen frame change automatically |
| focus at launch | current app keeps focus | frontmost stayed `访达` → `访达` |
| tray-only (no Dock icon) | accessory activation policy | `NSRunningApplication.activationPolicy = accessory`; System Events `background only = true` (bundled `.app` **and** bare binary) |
| menu-bar icon | dedicated template icon + attention variant | 72 px template PNGs shipped (`icons/tray.png`, `icons/tray-alert.png`); attention switch verified by `AGENT_MONITOR_DEBUG_TRAY=1` log `tray: switched to alert icon`; menu labels localized from `AppleLocale=zh_CN`; left-click menu disabled so left click belongs to the pet |
| typography | normal system fonts | pixel webfont removed; UI uses SF Pro / PingFang SC stack (see screenshots) |
| macOS material / appearance | native vibrancy + light/dark | `windowEffects: popover` (radius 16) applied; CSS uses `light-dark()` tokens; no runtime errors; dark/light previews rendered from the shipped HTML/CSS (native window screenshots are unavailable without Screen Recording permission) |
| row hierarchy | title-first, one state dot | title on line 1; agent/state/summary on line 2; no uppercase labels or middle-dot meta strings |

UI acceptance is automated headlessly (`npm run ui-test`, Chrome drives real DOM
events in the shipped page): **ALL PASS, 24 checks**, including:

* rows = real task count, six fixture agents/statuses rendered with distinct glyphs;
* list order follows the snapshot order (newest activity first);
* agent colours are deterministic per `agent_key` (no hard-coded agent names);
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

## Phase 3b — visual redesign (macOS popover)

Native window appearance cannot be captured on this host (Screen Recording
permission is not granted to the probe shell), so the material is verified in two
ways (the final geometry check was also done through the app's own
`AGENT_MONITOR_DEBUG_GEOMETRY` log because the display was locked): (a) the release app launches with `windowEffects` applied and no WebKit/
winit errors; (b) the shipped HTML/CSS is rendered in a single document over a
desktop-like backdrop (`?mock=1&desktop=1&appearance=dark|light`, design preview
only), which is exactly how the translucent CSS composes over the native material
view. Screenshots: `screenshots/pet-expanded-{dark,light}.png` and
`pet-collapsed-{dark,light}.png`.

Design changes: system font stack, native popover material, semantic macOS
colours, title-first rows, one coloured state dot + text label, hairline
separators, no uppercase labels, no middle-dot meta strings, light/dark support.

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

## Phase 5 — Settings window + configurability (0.4.1)

Commands below were executed on this host (macOS 27.0.1, arm64, 1920×1080 scale 1)
against the release build. UI interaction without Screen Recording/Accessibility
is verified with headless Chrome against the shipped pages (`?mock=1`) plus the
real window list / activation policy; those limitations are stated explicitly.

| Requirement | Result |
|---|---|
| Rust test suite | **Pass** — `cargo test --release` → **88 passed / 0 failed** (47 baseline tests preserved; +41 new: settings 14, adapters/monitor/retention 19, pet 4, attention freshness 2, icons 4 — overlapping counts differ slightly because some tests were replaced) |
| Headless UI | **Pass** — `bash scripts/ui-test.sh` → pet **69/69** (24 original + 15 settings integration + 3 pet settings + 4 dismiss-on-outside + 14 badge/icon/tab + 9 ring-badge/notice-dot) and settings **45/45** (37 + 8 icon/new_badge), first line `ALL PASS`, exit 0 |
| Single settings source | **Pass** — `Settings` in Rust, one JSON file, both windows + tray render from it; no localStorage language/polling after migration |
| Instant apply, no restart | **Pass (contract + headless)** — `settings-changed` drives both windows; scheduler woken through the wake channel. Real clicks not executed (see §Unexecuted 6) |
| Persistence across restart | **Pass** — `max_visible_rows` 3/5/8 written to `settings.json` and each fresh launch produced the matching window height; pet hidden state restored (edge-hidden 14 px sliver after relaunch; tray-hidden stayed hidden) |
| `language` affects pet + settings + tray | **Pass (headless + code path)** — pet switches en↔zh live from `settings-changed`; settings window 133 en/133 zh keys and re-renders; tray menu/tooltip rebuilt via `update_tray_menu` on language change. Real menu not clicked (no AX) |
| `appearance` | **Pass** — `Window::set_theme` available in tauri 2.11.2 and applied to both windows; explicit light/dark previews below; explicit override also sets page `color-scheme` |
| `refresh_interval` bounds | **Pass** — explicit 3/5/10 s validated client- and server-side (`MIN_REFRESH_SECS=2`), hidden/empty stays at the 30 s floor (unit tested via `Settings::active_refresh_secs` + worker logic) |
| `retention_hours` + active exemption | **Pass** — see the `--dump-tasks` comparison below: with 1 h retention all 5 running tasks stayed; finished rows older than 1 h disappeared. Unit tests cover Running/WaitingInput/WaitingApproval exemption and the inclusive boundary |
| `enabled_agents` stops collection | **Pass** — same comparison: disabling Codex + ZCode removed every codex/zcode row from the snapshot (collector registry rebuilt, not post-filtered) |
| `max_visible_rows` geometry | **Pass** — real `CGWindowList` bounds: rows 3 → `360×262`, rows 5 → `360×390`, rows 8 → `360×582`, right-anchored (`x=1560`), all inside the 1920×1080 visible frame; formula `44 + rows×64 + 26` exact |
| `collapsed_size` (48–96 px) | **Pass** — real `CGWindowList` bounds with `settings.json`: 48 → `48×48`, 72 → `72×72`, 96 → `96×96`, all right-anchored (`x=1848/1872/1824`) inside the visible frame; CSS variable keeps the mock identical (`--collapsed-size`) |
| `auto_hide_after_s` (hide after snapping) | **Pass (headless + command)** — with 1 s configured the pet hides exactly once after going idle on an edge (`hide_pet` called once); with `0` (default) it never hides; the underlying `hide_pet` path is the same one verified for edge-sliver/tray hiding above. Hover/drag/expand cancel the one-shot timer |
| Retention actually applies (bug fix) | **Pass** — with `retention_hours=72` the real snapshot went from **123 tasks / 8 terminal rows older than 72 h** (dsh 5, zcode 3) to **115 / 0**. Root causes: DSH kept rows because its projection cache files were rewritten on app start (fresh mtime) while the real `lastMessageTime` was 3.5–17 days old; ZCode kept the newest 3 unconditionally. DSH now uses real activity when known and falls back to mtime only when activity is unknown; ZCode's fallback was removed. Consequence (documented): ZCode contributes 0 rows until a task is younger than the configured window |
| Edge-hidden small tab | **Pass** — real `CGWindowList`: hidden window is **14×14 at x=1906** on a 1920-wide screen (flush right edge; snapped side invisible, perpendicular sides expose only 14 px), tab centre keeps the pet's along-edge position (`y=464` for a pet centre at 471); second launch recalls a 72×72 pet at `1848,435`, i.e. expanding grows from the tab. `hidden_sliver` 8–24 drives the square size |
| New-completion indicator | **Pass (headless + Rust)** — `tasks-updated` is now emitted while hidden so the badge can update; the collapsed badge is an unfilled green ring (16 px, 8 % tint, 1 px border) so it does not compete with the solid blue attention badge; checks cover count/clear-on-expand/`new_badge=false`/ring style |
| Jump notice never inside the small pet | **Pass** — the toast is state-driven and only painted while expanded/dragging; collapsing hides it and shows a text-free dot (amber/red/blue by kind, top-left), keeps the notice alive for ~30 s and announces it via `aria-label`/`title`; expanding restores the full toast with its action button. Headless checks cover hide-on-collapse, dot colours, restore-on-expand, lifetime and expiry |
| Custom icon framework | **Pass (unit + headless)** — `icons.rs` unit tests: PNG roundtrip, non-image rejected, >2 MB rejected, unknown slot rejected, PNG/JPEG/WebP sniffing only; settings-page checks cover upload/preview/remove/oversize rejection/`icons-changed`. Spec: `docs/ICONS.md` |
| Click outside collapses | **Pass (headless + code)** — explicit click expands with `focus=true`, hover expansion with `focus=false`; a window `blur` (click on another app/desktop/tray menu) calls `request_collapse(force=true)` and collapses immediately; mouse-leave always schedules collapse (the old suppression window can no longer drop it). Real pointer click needs AX/screen permission (§Unexecuted 6) |
| Stale “pending” clears | **Pass** — real data had a 59 h old Codex `interrupted` turn counted as failed/attention; after the fix `--dump-tasks` reports `attention_count: 0` while the row stays visible. Unit tests cover: interrupted/aborted/cancelled never count, failures only <30 min, waiting states always count |
| `edge_hide_enabled=false` | **Pass** — “Hide” orders the window out (`windows=none-visible`), restart preserves that state, second launch restores the pet; tray “Show Pet” uses the same `show_pet` path |
| Settings window | **Pass** — created lazily on first open (idle RSS stays at the pre-settings baseline until used); real probe while open: separate native window `620×560` at layer 0 next to the pet; app still `activationPolicy=accessory`, `backgroundOnly=true`; LSUIElement=true in the bundle |
| No Dock icon / accessory | **Pass** — bundled `.app`: `activationPolicy=accessory backgroundOnly=true name=Agent Monitor bundle=com.agent.monitor`, pet 72×72 at the right edge |
| Corrupt `settings.json` | **Pass** — `{broken json!!` → app starts, prints `corrupt file backed up …settings.json.bak`, defaults used (`--dump-tasks` still returned 42 real tasks), broken file preserved verbatim in `.bak` |
| Screenshots | Settings dark/light/zh + icon section + refreshed pet dark/light + collapsed new-badge + edge tab: `screenshots/settings-{dark,light}.png`, `settings-zh.png`, `settings-icons-dark.png`, `pet-expanded-{dark,light}.png`, `pet-collapsed-new-badge-ring-dark.png`, `pet-collapsed-notice-dot-dark.png`, `pet-edge-tab-dark.png` |
| Legacy localStorage migration | **Pass (observed end-to-end)** — launching the bundled app on this profile (which still had v0.3.x webview storage) imported `am-lang=zh`: the real `~/Library/Application Support/agent-monitor/settings.json` now contains `"language":"zh"` and `"legacy_imported":true` (no legacy `pollMs` was stored, so `refresh_interval` stayed `auto`). `--dump-tasks` then reported `language=Zh`, proving the migrated value is live. Legacy keys are removed after import |
| Performance | See `docs/PERF.md` “0.4.1 settings re-check” (hidden / idle / expanded) |

### `--dump-tasks` before/after (real data, isolated data dir)

```
# defaults
settings: retention=6h rows=5 enabled=[codex=true dsh=true zcode=true claude_code=true]
total 42   agents {dsh:26, claude-code:8, codex:5, zcode:3}   statuses {completed:36, running:5, idle:1}

# settings.json: enabled_agents.codex=false, enabled_agents.zcode=false, retention_hours=1
settings: retention=1h rows=8 enabled=[codex=false dsh=true zcode=false claude_code=true]
total 12   agents {dsh:11, claude-code:1}   statuses {completed:7, running:5}
```

Both scans reported `stable=true` (no duplicate rows). The 5 running tasks survived
the 1 h retention window; disabled agents contributed zero records.

### Real window probes

`swift scripts/wins-probe.swift <pid>` (CGWindowList + NSRunningApplication):

```
# rows=3, expanded            semantics: 44 + 3×64 + 26 = 262
window#1 layer=5 x=1560 y=656 w=360 h=262
# rows=5, expanded            44 + 5×64 + 26 = 390
window#1 layer=5 x=1560 y=435 w=360 h=390
# rows=8, expanded            44 + 8×64 + 26 = 582 (inside 1080 pt frame)
window#1 layer=5 x=1560 y=420 w=360 h=582
# edge-hidden, sliver=14      visible width 1920-1906 = 14
window#1 layer=5 x=1906 y=930 w=72 h=72
# edge_hide_enabled=false, hidden
windows=none-visible
# settings window open (620×560) beside the pet (360×390)
window#1 layer=5 x=1560 y=435 w=360 h=390
window#2 layer=0 x=650  y=236 w=620 h=560
activationPolicy=accessory backgroundOnly=true
```

### Deferred / not executed in this phase

The two remaining GUI-interaction items are listed as **§Unexecuted 6–7** below
with exact reproduction steps; everything else in this table was executed.

---

## Phase 6 — pending-question attention + jump setup (0.5.0)

Everything below was executed on the test Mac with the release binary / bundled
app; no GUI-pointer TCC interaction was required.

### Automated gates

| Gate | Command | Result |
|---|---|---|
| Rust unit tests | `cargo test --manifest-path src-tauri/Cargo.toml` | **120 passed, 0 failed** (88 baseline + 32 attention/jump) |
| Pet UI (real DOM) | `scripts/ui-selftest.html` via `bash scripts/ui-test.sh` | **ALL PASS, 88 checks** |
| Settings UI (real DOM) | `scripts/ui-selftest-settings.html` | **ALL PASS, 60 checks** |
| Release build | `cargo build --release` | 0 warnings/errors |
| Bundle | `bash scripts/bundle-mac.sh` | `dist/Agent Monitor.app` 0.5.0, ad-hoc |

### Pending-question attention (real end-to-end, isolated data dir)

A synthetic DSH projection file was used under a temporary `HOME` (the real
`~/.dsh` was not touched): first a running turn, then the same file was rewritten
with one active `userQuestions.questions.active[]` entry while the app was
collapsed. The file watcher woke the scheduler and the new question was detected:

```text
# before (collapsed)                      # after the question appeared
window#1 owner=agent-monitor w=72  h=72    window#1 owner=agent-monitor w=360 h=134
activationPolicy=accessory                 activationPolicy=accessory
```

The panel auto-expanded (`auto_show_on_question`) without focusing, exactly the
documented `44 + 1×64 + 26 = 134` height. The same fixture proves the detector
seeds at launch (no alert for pre-existing questions) and deduplicates repeats.

### Jump self-check + outcome fix (real output)

`dist/Agent Monitor.app/Contents/MacOS/agent-monitor --jump-diagnostics`:

```jsonc
{
  "accessibility": { "granted": true,  "detail": null },
  "automation":    { "granted": true,  "detail": null },
  "window_access": { "granted": true,  "detail": null },
  "system_events": true,
  "app_bundle_path": "…/dist/Agent Monitor.app",
  "agents": [/* codex L3 running, dsh L1 running, zcode L1 stopped, claude-code L2 running */]
}
```

`--jump dsh:<session>` now reports `kind: success, requested: L1, achieved: L1,
reason_key: activated_app` (previously a permanent `degraded` toast); Codex and
VSCode window jumps still report L3/L2 with read-back verification. A denied
L2/L3 attempt now carries `permission_needed` + `next_step: "grant_permission"`.

### Deferred

* Clicking a macOS notification cannot deep-link into the task: `osascript
  display notification` has no click action. The banner is an attention channel;
  the auto-revealed pet is the actionable surface.
* Real Notification Center appearance/denial is a system-level toggle; the
  command is unit-tested and executed with `osascript` but a denied banner cannot
  be asserted headlessly.

---

## Phase 7 — project/session-precise jump + footer count (0.6.0, 2026-10-22)

### Automated gates

| Gate | Command | Result |
|---|---|---|
| Rust unit tests | `cargo test --manifest-path src-tauri/Cargo.toml` | **139 passed, 0 failed** (120 baseline + 19 for AX/jump matching) |
| Pet UI (real DOM) | `scripts/ui-selftest.html` | **ALL PASS, 96 checks** (88 + 8 footer/visible-count checks) |
| Settings UI (real DOM) | `scripts/ui-selftest-settings.html` | **ALL PASS, 60 checks** (session-route labels/mock updated) |
| Release build | `cargo build --release` | 0 warnings / 0 errors |
| Bundle | `bash scripts/bundle-mac.sh` | `dist/Agent Monitor.app` 0.6.0, ad-hoc |

### DSH session-level jump (real app, 3 round-trips)

New native AX/CoreGraphics bridge (`src-tauri/src/ax.rs`; no crates, no subprocess
helper). Sidebar rows are scored on title + project (cwd basename) + relative
time; the matched row is clicked and the focused window title is read back.

```text
--jump dsh:session-0bfd41d5… ("# 给 agent-monitor 增加“设置页 +")
  before: 多窗口询问提醒与跳转 — DeepSeek Harness
  after : # 给 agent-monitor 增加“设置页 + — DeepSeek Harness
  outcome: success / L3 / opened_session      (3/3 round-trips, row stable after 2 reads)

--jump dsh:session-9ab2d0aa… (this conversation)  -> success / L3, back to 多窗口…
already-visible session                            -> success / L3, no click
subagent session not rendered in the sidebar       -> degraded / L1 / session_row_not_found
                                                      (window unchanged — never a guessed click)
```

`--ax-rows <pid>` prints trusted state, focused window title, window titles and
every sidebar row with frames — the read-only verification aid.

### VSCode project-precise window jump (real, two windows)

Two VSCode windows open, the other project focused:

```text
focused before: Welcome — AgentMonitor
--jump claude-code:625572ae…  -> success / L2 / opened_window
detail: arr_submission.pdf — DropDPO-Confidence
focused after : arr_submission.pdf — DropDPO-Confidence
```

`code --reuse-window <workspace_path>` (resolved from `/opt/homebrew/bin/code`)
selects the right project window; AX raise remains the fallback when the CLI is
missing.

### Footer count

With 6 sessions and `max_visible_rows=5` the expanded footer reads
`5 / 6 sessions` (`5 / 6 个会话`) with the hint “Showing 5 of 6 sessions”; with
rows=8 it reads `6 sessions`; with 20 sessions at rows=5 it reads
`5 / 20 sessions`; an empty snapshot reads `0 sessions`. Covered by 8 real-DOM
checks in the pet suite.

### Deferred / known limits

* DSH sidebar AX exposes only rendered rows (virtualized list); a row outside the
  viewport degrades honestly instead of scrolling automatically.
* Identical titles that also share project and time bucket are reported as
  `session_ambiguous` rather than clicking one of them.
* The DSH AX route depends on Electron's accessibility tree; a DSH update could
  change the tree, which is why every jump verifies the window title before
  claiming L3.

---

## Phase 8 — in-app updates, visible new completions, status dots (0.7.0, 2026-10-22)

### Automated gates

| Gate | Command | Result |
|---|---|---|
| Rust unit tests | `cargo test --manifest-path src-tauri/Cargo.toml` | **151 passed, 0 failed** (142 baseline + updater/settings tests) |
| Pet UI (real DOM) | `scripts/ui-selftest.html` | **ALL PASS, 129 checks** (96 → 129: row markers, status dots, update notice, soft-warning dedupe) |
| Settings UI (real DOM) | `scripts/ui-selftest-settings.html` | **ALL PASS, 81 checks** (60 → 81: Updates pane, events, progress, restart) |
| Release build | `cargo build --release` | 0 warnings / 0 errors |
| Bundle | `bash scripts/bundle-mac.sh` | `dist/Agent Monitor.app` 0.7.0, ad-hoc |

### In-app updater — real end-to-end swap (signed with the repo test key)

A copied 0.7.0 `.app` was pointed at a local `latest.json` (signed 0.7.1
artifact) and launched with the debug auto-install path:

```text
$ …/--check-update   (old 0.7.0 bundle, local endpoint)
{"current_version":"0.7.0","available":true,"version":"0.7.1",
 "notes":"Round-4 local updater verification","pub_date":"2026-10-22T09:45:31Z"}

$ AGENT_MONITOR_UPDATE_AUTO_INSTALL=1 … (old bundle)
update: installed 0.7.1; restarting
update: auto-install: already at 0.7.1 (nothing to do)

$ plutil -p "/private/tmp/am-final/old/Agent Monitor.app/Contents/Info.plist"
  "CFBundleShortVersionString" => "0.7.1"
```

The tarball was signed with `tauri signer` (minisign) and the plugin verified
the signature before swapping the bundle in place; the restart re-executed the
new binary and the follow-up check reported up to date. The real GitHub endpoint
correctly reports a readable error until a published release carries
`latest.json` (remote latest is still v0.3.1).

### Pet: identify new completions + three status dots

* On open, rows completed since the last look get a green “Newly completed”
  marker; the set is frozen for the open session (late completions join without
  flicker) and cleared on collapse. The header chip and collapsed ring still show
  the count.
* Collapsed top-left status cluster: yellow (waiting question), green (new
  completions), red (failed/interrupted) can be visible simultaneously; the
  jump-notice dot moved to the bottom-right. Rect-based selftests prove no
  collisions at collapsed sizes 48/72 and in the 14 px edge tab (single
  highest-priority dot).
* Soft jump notices are deduped per `task_id:reason_key` (one per session);
  failures always report; update warnings are unaffected.

### Jump reliability (round 3, see `docs/JUMP.md`)

* DSH sessions in collapsed projects now succeed at L3 (workspace auto-expand,
  verified: 2.4 s, window title read-back).
* Verification polling removes false `focus_unconfirmed` / `deep_link_unverified`
  warnings for slow-but-successful jumps (normal Codex 0.6 s, VSCode 1.5 s).
* Codex deleted-thread fallback is honest (`degraded / L1`) and 35 % faster
  (7.1 s → 4.6 s); the pet shows the soft notice only once per task.

### Deferred

* macOS first-run TCC prompts and Notification Center appearance still require a
  graphical session (documented in earlier phases).
* The updater downloads the full ~9 MB signed artifact (no binary deltas), and
  the app must be restarted after install (the UI offers “Restart Now”).
* Updating replaces an ad-hoc-signed bundle, so macOS may ask for
  Accessibility/Automation again; Settings → Jump shows the live state.

---

## Phase 9 — capsule polish + permission repair (0.7.1, 2026-10-22)

### Automated gates

| Gate | Command | Result |
|---|---|---|
| Rust unit tests | `cargo test --manifest-path src-tauri/Cargo.toml` | **155 passed, 0 failed** (151 baseline + AX prompt / tccutil safety tests) |
| Pet UI (real DOM) | `scripts/ui-selftest.html` | **ALL PASS, 136 checks** (129 → 136: no collapsed count ring, macOS-style capsule, breathing/dots, layout) |
| Settings UI (real DOM) | `scripts/ui-selftest-settings.html` | **ALL PASS, 101 checks** (81 → 101: collapsible guide, app location, stale-grant alert, request/reset, focus refresh) |
| Release build | `cargo build --release` | 0 warnings / 0 errors |
| Bundle | `bash scripts/bundle-mac.sh` | `dist/Agent Monitor.app` 0.7.1, ad-hoc |

### Pet: ring removed, macOS-style status capsule

* The collapsed green new-completion count ring is gone (element, CSS, JS and
  aria text removed; zero `new-badge` references in `src/`). The expanded header
  chip and per-row green “newly completed” markers remain, gated by `new_badge`.
* The three semantic dots are now a small translucent traffic-light capsule
  (hairline border, inset highlight, soft shadow, **no** `backdrop-filter`),
  with 6 px glass-bead dots (radial gradient + inner highlight + darker rim).
  Only the amber dot breathes (2.6 s, scale 1→1.06 + glow) while a question is
  pending; `animations=off` / Reduce Motion keep it static. The 14 px edge/tray
  tab shows a single 3 px highest-priority dot. Rect-based checks prove no
  collisions with the top-right badge, glyph, bottom-right notice dot or edges at
  collapsed_size 48/72/96 in light and dark.

### Settings → Jump permission UX

* First-run/ad-hoc guidance is now a collapsed native `<details>` (summary shows
  “Permissions needed (N) / 需要授权（N 项）” when something is missing; status
  rows stay visible). The pane also shows the running **App location**
  (`get_jump_diagnostics.app_bundle_path`) and an amber stale-grant alert when
  Accessibility is missing.
* New repair actions: **Request permission** (`request_accessibility_permission`,
  native `AXIsProcessTrustedWithOptions` prompt in the app's own process) and an
  inline-confirmed **Reset this app's permissions…** (`reset_app_permissions`,
  runs `tccutil reset Accessibility|AppleEvents com.agent.monitor` only). The
  argv builder refuses any call without a bundle id (unit-tested).
* Returning focus to the Settings window re-runs the self-check while the Jump
  pane is selected (concurrent events collapse into one request, never during a
  command), so toggling the System Settings switch updates the pills.
* Debug aids added: `AGENT_MONITOR_DUMP_JUMP_DIAGNOSTICS=<path>` (writes the
  diagnostics JSON from the real packaged app) and `AGENT_MONITOR_REQUEST_AX_PROMPT=1`
  (in-process AX prompt at startup). Verified with `dist/Agent Monitor.app`:
  `app_bundle_path` points at the real bundle, `automation.granted=true`,
  `accessibility.granted=false` (see the incident below).

### Incident: machine-wide Accessibility reset during testing

While probing the `tccutil` output format for the new reset command, a shell test
ran `/usr/bin/tccutil reset Accessibility` **without a bundle id**. On macOS this
clears the Accessibility TCC records for **all applications**, not just this one.
Automation/AppleEvents was not touched. Consequences and handling:

* Accessibility grants cannot be restored programmatically; the user must
  re-enable apps in System Settings → Privacy & Security → Accessibility
  (terminal/harness, Agent Monitor, and any other AX tooling they rely on).
* The real in-app diagnostics dump now correctly reports
  `accessibility.granted=false`; before this incident the same host had it
  granted. Window-level jump re-verification for this round therefore could not
  be executed (earlier round-4 DSH/VSCode evidence still stands); Settings → Jump
  can re-verify once the user re-grants.
* Hardening: the shipped reset command is restricted to
  `tccutil reset <service> com.agent.monitor` with a unit test asserting the
  bundle id is always the last argv element and that blank service/bundle ids are
  refused; both `docs/UPDATING.md`/`docs/JUMP.md`/`docs/SETTINGS.md` warn never to
  run tccutil without a bundle id.

### Deferred

* Re-grant Accessibility after the incident, then use Settings → Jump → Request
  permission / Check again to confirm the pills flip to “Granted”.
* macOS first-run TCC prompts and Notification Center appearance still require a
  graphical session.

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
4. **Terminal tab L3 (future, not routed)**
   Claude Code is modelled as VSCode-hosted, so no current task uses the Terminal
   path. If a terminal-hosted agent is added later, run it in Terminal and click
   its task: expected AppleScript selects the tab by tty and reports `success/L3`.
5. **ZCode jump**
   Launch `ZCode.app` successfully (it currently exits immediately), then retest
   `zcode://workspace/open?path=`.

6. **Settings UI pointer interaction, Cmd+, and tray menu**
   No Accessibility permission for the probe shell. Covered instead by headless
   DOM event tests (gear/Cmd+, → exactly one `open_settings`; control edits emit a
   full `set_settings` payload; reset double-confirm; Escape → `close_settings`),
   the real window/activation probes, and code review.
   Repro: grant Accessibility to Terminal, focus the pet, press Cmd+, edit a
   control, quit and relaunch; click tray → Settings… and footer gear.
7. **Settings window close button** (traffic-light) hiding instead of quitting.
   Code path `CloseRequested → prevent_close + hide` is compiled and the window
   is created lazily/single-instance; the actual click needs a GUI session.
   Repro: open Settings, click the red close button; app must stay in the menu
   bar and `activationPolicy` must remain `accessory`; reopen Settings.

8. **Custom-icon upload and click-outside dismissal with a real pointer**
   Both need Accessibility (or a GUI session) to drive; covered headlessly (file
   input → `save_custom_icon`, blur → collapse) plus Rust-side validation unit
   tests. Repro: open Settings → Desktop Pet → Icon, choose a 144×144 PNG, set
   Icon style = Custom, verify the pet changes immediately; then click another
   app and verify the expanded panel collapses.
