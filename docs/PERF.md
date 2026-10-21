# Performance measurements

**Environment (measured):** macOS 27.0.1 (26A434), Apple Silicon arm64, single
1920×1080 display, Rust 1.99.1, **release** build (`cargo build --release`,
Tauri 2.11.2 + custom-protocol + macOSPrivateApi). Idle system with the normal
desktop apps running (ChatGPT, DeepSeek Harness, VS Code, Terminal). The pet was
the only build of Agent Monitor running.

**Method** (`scripts/perf.py`, invoked by `scripts/perf.sh`):

* launch the release binary with a forced display mode
  (`AGENT_MONITOR_START_MODE=…`) or with `AGENT_MONITOR_EMPTY=1` for the
  “no tasks” case (measurement aid; skips adapters so the snapshot is truly empty);
* warm-up 8 s, then sample every 5 s for 300 s;
* CPU = delta of `ps -o cputime=` divided by wall-clock delta (true interval
  average, not the lifetime average printed by `top`); RSS from `ps -o rss=`;
* raw series: `docs/perf/<label>.csv`; summary JSON: `docs/perf/<label>.json`
  (each summary now carries `valid: true`; dead/replaced processes invalidate a run);
* process killed afterwards; no mock data is used except the empty-snapshot mode.

## Results — final build 0.4.1 (300 s per state, 60 samples each)

Measured after the visual refactor (system fonts), tray-only policy and
newest-first ordering. Each run is validated: the sampler aborts and marks the
run invalid if the process dies or its CPU counter goes backwards.

| State | Tasks in snapshot | CPU mean | CPU p95 | CPU max | RSS start → end | RSS growth |
|---|---|---|---|---|---|---|
| Edge-hidden (14 px sliver) | 40 real | **0.05 %** | 0.20 % | 0.20 % | 98.2 → 84.5 MB | **−13.7 MB** |
| Idle, no tasks (collapsed) | 0 (empty mode) | **0.03 %** | 0.20 % | 0.20 % | 92.3 → 92.2 MB | **−0.14 MB** |
| Expanded, 40 tasks (5 visible, scrollable) | 40 real | **0.21 %** | 1.00 % | 1.40 % | 97.8 → 90.0 MB | **−7.8 MB** |

Interpretation:

* Every state is comfortably below the suggested **< 1 % of one core** budget on
  average. The expanded state's p95 touches 1.0 % because the live DSH sessions
  produce snapshot updates and row re-renders; this is the intended active state,
  not idle overhead.
* RSS decreases in all three runs (allocator returning pages) — no continuous
  memory growth. The earlier 0.4.0 short idle run that showed +9.9 MB was
  WebKit warm-up; the 600 s run below confirms the trend reverses.
* Hidden state performs no UI updates and no animation; its fallback tick is 30 s
  and adapter scans are mtime-gated.

## Longer idle run (600 s, 120 samples, build 0.4.0)

The idle path is unchanged between 0.4.0 and 0.4.1 (same scheduler, same
adapters); this run is kept as the long-duration memory evidence.

`docs/perf/idle-empty-long.json`:

| State | CPU mean | CPU p95 | CPU max | RSS start → end | RSS growth |
|---|---|---|---|---|---|
| Idle, no tasks, collapsed, **600 s** | **0.043 %** | 0.20 % | 0.20 % | 92.3 → 80.1 MB | **−12.3 MB** |

Long-run conclusion: the short run's +9.9 MB was WebKit/JSC warm-up, not a leak —
over 10 minutes RSS decreases by 12.3 MB, and the CPU mean drops to 0.043 %. No
continuous memory growth was observed in any valid run. A full 30-minute run is
still listed under “Not measured” for completeness.

## Longer idle run (600 s, 120 samples)

`docs/perf/idle-empty-long.json`:

| State | CPU mean | CPU p95 | CPU max | RSS start → end | RSS growth |
|---|---|---|---|---|---|
| Idle, no tasks, collapsed, **600 s** | **0.043 %** | 0.20 % | 0.20 % | 92.3 → 80.1 MB | **−12.3 MB** |

Long-run conclusion: the short run's +9.9 MB was WebKit/JSC warm-up, not a leak —
over 10 minutes RSS decreases by 12.3 MB (allocator returning pages), and the CPU
mean drops to 0.043 %. No continuous memory growth was observed in either run.
A full 30-minute run is still listed under “Not measured” for completeness, but
the trend direction is downward.

## What the app does when hidden / idle (by design)

* File changes arrive via `notify` (FSEvents) with a 700 ms debounce; the periodic
  fallback is **5 s** while visible with tasks and **30 s** while hidden or empty.
* Every adapter checks its source’s mtime (or byte offset) before parsing; an
  unchanged source costs a stat + a string comparison, no JSON/SQLite read.
* The UI re-renders only when the snapshot’s render hash changes; relative times
  are refreshed by **one** 30 s timer that rewrites time labels only.
* No per-task timers, no screen capture, no terminal scraping, no network.
* Animations run only for the 150–200 ms expand/collapse transition; nothing
  animates while collapsed or idle, and `prefers-reduced-motion` is respected.

## Not measured

* CPU during continuous drag/scroll (interaction latency instead verified in the
  headless UI test: 20 rows render < 1 ms, panel height fixed, list scrolls).
* 30-minute continuous memory trend was **not** executed with a single 30-minute
  run; two runs of 5 and 10 minutes are recorded (10-minute RSS trend is
  decreasing). A 30-minute run can be reproduced with
  `python3 scripts/perf.py --mode collapsed --seconds 1800 --label idle-30m --empty`.
