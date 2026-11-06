# Pending-question attention (design + contract)

Problem: an agent can block on a question (`ask user question`, approval prompt)
while the user is focused in another window / Space. The pet already counts
"attention", but with many windows the user has to look at the menu bar in time.
An unanswered question can therefore sit for minutes.

Solution: detect **new** pending questions in the monitor, then surface them on
three independent channels:

1. **macOS notification** (best effort) — appears over whatever app is focused.
2. **Sound** (optional) — a short system sound.
3. **The pet reveals itself** (optional) — the always-on-top popover expands
   without stealing keyboard focus, so the waiting session is one click away.

The same new-question payload is pushed to the pet UI, which pulses the
attention badge and adds a "waiting for you" count to the header.

## Detection rules

A task is a **pending question** when its status is `waiting_input` or
`waiting_approval`. (Failures never trigger this channel; they keep the normal
attention rules.)

Source evidence:

* Codex: unanswered `request_user_input` / `request_user_input_async` calls in
  the current open turn. Results settle their own call; async `accepted: true`
  is not an answer. Replies settle individual question indices.
* Claude Code: parent `AskUserQuestion` remains pending until its matching
  `tool_result` or an explicit new user prompt. Unrelated results do not clear it.
* DSH: the explicit `userQuestions.questions.active` projection, taking
  precedence over open/closed turn status.
* ZCode: explicit task-index waiting statuses. Additional tool names require
  verified protocol evidence; prose mentioning “ask” is never a signal.

File-event bursts wake the scanner within a bounded 700 ms debounce window,
so continuous logging in other sessions cannot defer a new question indefinitely.

The monitor keeps a fingerprint of every currently visible pending question:

```
fingerprint(task) = (status, last_activity_secs)
```

A question is **new** when its id is absent from the previous fingerprint map,
or its fingerprint changed. A scan that reports the same waiting task with the
same activity does not alert again. A task that goes waiting → running →
waiting (a new question in the same session) alerts again because its activity
timestamp moves.

Re-alerts for a task that is **still pending** are rate-limited to once per
`ALERT_COOLDOWN_SECS` (60 s): some adapters can advance a waiting task's
activity timestamp without the user actually getting a new question, and a
tick inside the cooldown must not produce another banner/sound. A first
appearance or a status change into waiting is reported immediately.

Rules:

* The **first scan after launch only seeds** the fingerprint map: existing old
  questions do not produce a burst of notifications at startup.
* Dismissed tasks never alert (they are filtered before detection).
* Disabled agents are not collected, therefore never alert.
* Non-question statuses (`failed`, `running`, `completed`, ...) are ignored.
* At most `MAX_ALERTS` (8) questions are reported per batch; notification text
  collapses a batch into one notification.

## Event `attention-pending`

Emitted by Rust after a scan that produced at least one new question:

```jsonc
{
  "alerts": [{
    "task_id": "dsh:session-…",
    "agent_key": "dsh",
    "agent_name": "DSH",
    "title": "多窗口询问提醒与跳转",
    "status": "waiting_input",
    "status_reason": "1 active user question(s)"
  }],
  "question_count": 2,     // all visible waiting_input + waiting_approval tasks
  "auto_show": true        // the configured auto_show_on_question value
}
```

`question_count` is also a field of every `Snapshot` (see `docs/UI_CONTRACT.md`),
so a window that just opened can paint the chip without waiting for an event.

## Settings

New group `attention` (`reset_settings scope=attention`):

| key | type | default | meaning |
|---|---|---|---|
| `notify_on_question` | bool | `true` | show a macOS notification when a new question appears |
| `alert_sound` | bool | `true` | play the system "Glass" sound with the alert |
| `auto_show_on_question` | bool | `true` | reveal/expand the pet when a new question appears |

Per-agent switches and `new_badge` are independent; the attention channel only
concerns blocking questions.

## Notification details

* macOS: `osascript -e 'display notification … with title … [sound name "Glass"]'`
  is used, so no extra binary/plugin is required. macOS may attribute the banner
  to the terminal/app that launched the helper; if notifications are denied in
  System Settings nothing is shown, and the pet/sound channels still work.
* The text is localised from the Settings language; the body is
  `"<agent> · <title>"`, the title is `等待你的回答` / `Waiting for your answer`.
* A batch of N questions sends one notification (`N 个会话在等你回答`), not N.
* `alert_sound=true` with `notify_on_question=false` plays the sound with
  `afplay` so an unsilenced alert is still possible.
* All of this runs on a detached thread: a missing `osascript`/`afplay` or a
  denied banner can never block the scheduler or the UI.

## Auto-reveal details

When `auto_show_on_question` is on and the pet is `collapsed`, `edge_hidden` or
`tray_hidden`:

1. show the window if it was ordered out (`show_pet` semantics);
2. expand it (`enter_mode(Expanded)`) **without** focusing it — the user may be
   typing in another app; hover/click focus rules are unchanged;
3. the panel naturally shows the waiting rows first (newest activity ordering)
   with the amber "waiting for you" chip.

An already expanded panel is left alone. The UI may also receive the event in
the mock/browser; the packaged behaviour is owned by Rust so a collapsed
webview that missed an event is still revealed.

## Pet UI surfaces

* `#badge` (top-right attention count) switches to the amber "question" variant
  while `question_count > 0` and gains a one-shot pulse whenever
  `attention-pending` arrives. `animations=off` / Reduce Motion disables the
  pulse.
* `#header-meta` keeps the total attention count; a new amber chip
  (`#header-question`) shows `N 待回答` / `N waiting` while the panel is open.
* The collapsed pet's `aria-label`/`title` includes the waiting count.
* Rows keep the existing status dot + text; no colour-only signal.

## Verification

```bash
cargo test --manifest-path src-tauri/Cargo.toml        # attention detector + settings
bash scripts/ui-test.sh                               # pet chip/pulse + settings pane
# manual: with a real session asking a question, watch a banner + pet expansion;
# `~/Library/Application Support/agent-monitor/settings.json` carries the 3 keys.
```

Debug aid: `AGENT_MONITOR_SIMULATED=1` includes a `waiting_approval` fixture (it only
seeds the first scan, so it shows up in `question_count` rather than firing an
alert); `--dump-tasks` prints `question_count`.
