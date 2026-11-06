# UI contract — desktop pet frontend

The frontend is plain static HTML/CSS/JS (no bundler). It talks to Rust only
through the commands below. All agent-specific logic stays in Rust; the UI
renders whatever the snapshot contains.

## Snapshot JSON

```jsonc
{
  "tasks": [{
    "id": "codex:01a15a27-…",        // stable id, use as DOM key
    "agent_key": "codex",             // stable key; colour mapping with fallback
    "agent_name": "Codex",            // display name (never hard-code a list)
    "host": "desktop",                // desktop | vscode | cli | terminal
    "session_id": "01a15a27-…",
    "task_id": null,
    "title": "Fix the parser",
    "status": "running",              // running|waiting_input|waiting_approval|completed|failed|idle|unknown
    "status_reason": "task_started",  // evidence, shown in tooltip/detail
    "last_activity_secs": 1792579593, // epoch seconds of real activity
    "collected_at_secs": 1792580000,
    "summary": "Editing parser.rs",   // may be null
    "model": "gpt-6.1-sol",           // latest recorded model, may be null
    "origin": "event",                // event|index|cache|synthetic
    "target": {
      "app_name": "ChatGPT",
      "bundle_id": "com.openai.codex",
      "deep_link": "codex://threads/…",   // may be null
      "window_title": "Fix the parser",   // may be null
      "workspace_path": "/path/to/ws",    // may be null
      "terminal_host": null,
      "terminal_tty": null,
      "host_ref": null,
      "process_id": null
    },
    "target_validity": "valid",       // valid|invalid|unconfirmed
    "invalid_reason": null,           // string when invalid
    "capability": { "max_level": "L2", "notes": "…", "requires": ["accessibility"] },
    "level": "L2",                    // advertised effective level
    "pinned": false
  }],
  "collected_at_secs": 1792580000,
  "attention_count": 1,            // waiting_* always; failed only if recent (<30 min) and not interrupted/aborted/cancelled
  "question_count": 1,             // waiting_input + waiting_approval only (never failures)
  "agents": [{ "key": "codex", "name": "Codex", "source": "…", "origin": "index", "max_level": "L2", "notes": "…" }]
}
```

`model` is the full latest recorded model id or `null` when the source cannot
establish it. The row renders it independently of progress-summary visibility.
Codex uses the latest turn context; DSH uses `modelSelection.lastUsed` (never a
pending selection); Claude Code uses the latest parent assistant model; ZCode
exposes the task-index model, whose source does not identify a separate turn.

The pin action calls `set_task_pinned(task_id, pinned)`. Pins persist in
`pins.json`, precede unpinned tasks, and bypass normal retention filtering.
Each group remains sorted by most recent activity. Pin/unpin never jumps to the
conversation or acknowledges its completion.

## Commands (Tauri `invoke`)

| Command | Args | Returns | Notes |
|---|---|---|---|
| `get_snapshot` | — | Snapshot | initial paint |
| `watch_unread_completions` | `completions: [{task_id, completion_id}]` | — | derived unread versions for native receipt / foreground checks; read state remains in the webview |
| `refresh` | — | Snapshot | explicit user refresh |
| `jump_to_task` | `task_id: string` | `Result<JumpOutcome, String>` | **exactly one** jump per click; rejection only on transport failure |
| `dismiss_task` | `task_id: string` | `boolean` | remove stale/invalid record |
| `get_pet_state` | — | PetStateView | mode/edge/hidden/settings |
| `set_expanded` | `expanded: boolean, focus?: boolean` | — | resize; `focus=true` only for explicit clicks (hover expansion never focuses) |
| `request_collapse` | `force?: boolean` | — | `force=true` for explicit dismissal (outside click/blur/Escape), bypasses the post-resize suppression window |
| `open_permission_settings` | `kind: "accessibility"\|"automation"` | — | system pane |
| `get_jump_diagnostics` | — | JumpDiagnostics | permission + per-agent jump self-check (see `docs/JUMP.md`) |
| `reveal_app_in_finder` | — | Result | reveal the .app bundle so it can be added to Accessibility |
| `request_accessibility_permission` | — | `Result<bool, String>` | native AX prompt for the current app; `true` = already trusted, `false` = prompt shown |
| `reset_app_permissions` | — | `Result<string, String>` | `tccutil reset Accessibility/AppleEvents` for **this bundle id only** (never machine-wide); returns a human-readable summary |
| `check_for_update` | — | UpdateCheck | in-app updater check (`{current_version, available, version?, notes?, pub_date?}`) |
| `install_update` | — | Result | download + verify + install the update in place; emits progress events |
| `restart_app` | — | — | restart to finish an installed update |
| `get_settings` | — | Settings | single source of truth (see `docs/SETTINGS.md`) |
| `set_settings` | `settings: Settings` | Settings (normalized) | instant save + broadcast `settings-changed` |
| `reset_settings` | `scope: "all"\|"general"\|"attention"\|"monitor"\|"pet"\|"jump"\|"update"` | Settings | group defaults |
| `get_settings_meta` | — | SettingsMeta | data paths / version / system language |
| `import_legacy_settings` | `lang?`, `pollMs?` | Settings \| null | one-shot v0.3.x migration |
| `clear_dismissed` | — | number | clear ignored tasks (persisted) |
| `restore_dismissed_task` | `taskId` | boolean | undo auto-dismiss |
| `open_settings` / `close_settings` | — | — | single settings window |
| `open_data_dir` | — | Result | reveal data folder in Finder |
| `get_custom_icons` | — | `{collapsed, hidden}` | custom icon data URLs (see `docs/ICONS.md`) |
| `save_custom_icon` | `slot, dataUrl` | IconMeta \| error | validated (magic bytes, ≤2 MB) + atomic write |
| `clear_custom_icon` | `slot` | Result | remove a slot |
| `begin_drag` | — | — | JS calls `getCurrentWindow().startDragging()` |

JumpOutcome:

```jsonc
{
  "task_id": "…",
  "kind": "success|degraded|failed|unconfirmed",
  "requested": "L3",
  "achieved": "L2",
  "reason_key": "window_focused_no_session",  // UI maps to short localised text
  "detail": "ChatGPT",
  "next_step": "grant_accessibility",
  "permission_needed": "accessibility"        // may be null
}
```

PetStateView:

```jsonc
{
  "mode": "collapsed|expanded|dragging|edge_hidden|tray_hidden",
  "edge": "left|right|top|bottom|none",
  "hidden": false,
  "hidden_to_tray": false,           // ordered out instead of parked at the edge
  "settings": { /* full Settings object, see docs/SETTINGS.md */ }
}
```

Settings (abridged; authoritative list in `docs/SETTINGS.md`):

```jsonc
{
  "version": 1,
  "language": "system|en|zh",
  "appearance": "system|light|dark",
  "show_summary": true,
  "show_jump_level": true,
  "new_badge": true,
  "refresh_interval": "auto|3s|5s|10s",
  "retention_hours": 6,
  "enabled_agents": { "codex": true, "dsh": true, "zcode": true, "claude_code": true },
  "max_visible_rows": 5,
  "hover_expand_ms": 150,
  "collapse_delay_ms": 800,
  "snap_distance": 16,
  "hidden_sliver": 14,
  "collapsed_size": 72,
  "auto_hide_after_s": 0,
  "edge_hide_enabled": true,
  "icon_style": "builtin|custom",
  "animations": "system|always|off",
  "notify_on_question": true,
  "alert_sound": true,
  "auto_show_on_question": true,
  "auto_check_updates": true,
  "toast_on_degraded": true,
  "auto_dismiss_invalid": false,
  "legacy_imported": false
}
```

`completion-viewed` carries `{task_id, completion_id}` for an exactly identified
foreground conversation or a native Codex read receipt. Codex receipts require
`first_unread_turn IS NULL` and `last_published_turn` matching the completed
rollout's turn ID. They work after the user switches away and require no AX
permission. Missing receipts or an older published turn do not acknowledge the
answer. Events acknowledge that version only; delayed events cannot acknowledge
newer completions. Desktop receipt checks use the existing active refresh interval even while the
pet is hidden. Codex CLI reads are checked from the existing pet heartbeat (~1 s):
only the foreground Terminal tab's TTY is queried, then its attached processes'
open rollout files identify the conversation. An exact, unique session and
completion version are required, and the selected tab is rechecked before the
read event. OS queries run off the animation thread, with one probe at a time;
no new task-scan or frontend timer is added. This Terminal route requires
Automation access to Terminal; inaccessible/ambiguous tabs remain unread.

Events: `tasks-updated` (Snapshot; emitted even while hidden so the collapsed
“new completions” indicator updates), `pet-state` (PetStateView), `jump-feedback`
(JumpOutcome, used for tray or keyboard initiated jumps), `settings-changed`
(Settings; broadcast to both windows, never polled), `icons-changed`
(`{collapsed, hidden}` data URLs), and `attention-pending` — emitted only when a
session **newly** blocks on a question:

```jsonc
{
  "alerts": [{
    "task_id": "dsh:session-…", "agent_key": "dsh", "agent_name": "DSH",
    "title": "…", "status": "waiting_input", "status_reason": "1 active user question(s)"
  }],
  "question_count": 2,
  "auto_show": true
}
```

The pet pulses its amber question badge, the header shows the waiting count, and
Rust separately shows a macOS notification / plays a sound / auto-reveals the
panel. Detection, dedupe and settings: `docs/ATTENTION.md`.

In-app updater events (see `docs/UPDATING.md`):

| Event | Payload |
|---|---|
| `update-available` | `{ current_version, available: true, version, notes, pub_date }` (startup auto-check only) |
| `update-progress` | `{ downloaded, total \| null }` |
| `update-installed` | `{ version }` — restart still required |
| `open-update-pane` | — (tray → Settings, select the Updates pane and check) |

Dev-only: with `?mock=1` (or `?mock=collapsed`) the pet page renders a fixture and
exposes `window.__AM_TEST__` for `scripts/ui-selftest.html`. The settings page uses
`settings.html?mock=1&appearance=light|dark&lang=en|zh&group=general|monitor|pet|jump|data`
and exposes `window.__AM_SETTINGS_TEST__` for `scripts/ui-selftest-settings.html`.
`bash scripts/ui-test.sh` runs both suites. The packaged app never loads the mock path.

## Presentation rules (current)

* **macOS material**: the window uses Tauri `windowEffects: popover` (real
  NSVisualEffectView vibrancy, radius 16). CSS only adds a translucent tint; it
  must not add `backdrop-filter` (it samples black over a transparent window).
* **Appearance**: `color-scheme: light dark` + `light-dark()` tokens; the popover
  follows the system appearance. Light and dark are both first-class.
* **Tray-only app**: accessory activation policy, no Dock icon, no app-switcher
  entry. The menu-bar item is the only entry point.
* **System typography**: SF Pro / PingFang SC stack; no webfont is loaded.
* **Row hierarchy**: title on the first line (13 px, primary); agent name
  (secondary), one status dot + label, and an optional short summary on the second
  line; time + jump level right-aligned. No uppercase labels and no `A · B · C`
  middle-dot meta strings (summaries use commas).
* **Ordering**: pinned conversations precede unpinned ones; each group sorts by
  `last_activity_secs` descending. The UI renders snapshot order and must not
  re-group by status; attention is surfaced by the header count and the coloured
  state dot/Label.
* **Footer count**: the expanded footer shows how many sessions the current window
  can display, not just the total: `visible = min(tasks.length, max_visible_rows)`
  (the same rule Rust uses for the window height). When some rows scroll, render
  `"{visible} / {total} sessions"` (e.g. `5 / 119 个会话`); when everything fits,
  render `"{total} sessions"`. A localized `title`/aria hint explains
  "Showing V of T sessions".

## Interaction rules implemented by the frontend

* Collapsed: translucent squircle with a three-bar glyph and a blue count badge;
  click expands. The edge length is `collapsed_size` (48–96 px, default 72) and
  the corner radius scales with it; the native effect radius stays 16 px.
* Expanded: fixed max height (`max_visible_rows` visible, scroll for more) with
  one row per task showing agent name, title (ellipsis + `title` tooltip), status
  text **and** icon (not colour alone), relative last-activity time, and
  jump-level hint.
* “New completions”: read state is stored per task id and stable completion identity in
  localStorage (`am-completion-reads-v2`). Opening, closing, hovering, refreshing or
  restarting the panel never acknowledges a completion. A confirmed successful
  row jump acknowledges only the clicked completion version. Manually viewing a
  conversation also acknowledges that version when macOS Accessibility identifies
  the foreground application by NSWorkspace PID/bundle id and its conversation
  by session reference or exact unique title/selected
  conversation. App/workspace activation alone, ambiguous titles or unavailable
  permissions leave the marker intact. Failed, degraded,
  unconfirmed and transport-failed jumps retain the marker unless an exact foreground
  conversation match independently confirms viewing. Metadata changes never create a new completion.
  Answers without a source completion identity do not create unread alerts. A later completion
  of the same task becomes unread again. Initial installation starts tracking
  from that moment, without marking the existing history unread. The v2 read
  store establishes this baseline once on upgrade because old activity-based
  receipts cannot reliably identify answer versions. There is no
  collapsed count ring; the expanded header counts unread completions and each
  unread completed row carries a green dot + “newly completed” label. Clicking
  that dot explicitly marks just that answer as read without jumping. The
  collapsed pet keeps its semantic green dot until no unread completions remain.
  `new_badge=false` hides the header chip, the row markers and the green dot.
  The footer offers “Mark all as read” while unread completions exist. One click
  acknowledges all currently unread completed versions and saves them without
  opening conversations. Running tasks and pending questions are untouched;
  subsequent completions become unread again.
* Jump notices never render inside the small pet window: the toast is painted only
  while the panel is expanded/dragging. While collapsed/edge-hidden/tray-hidden a
  text-free notice dot (**bottom-right**; amber = degraded, red = failed, blue =
  info) is shown instead, the notice lifetime is extended (~30 s), and expanding
  restores the full toast with its action button. The pending notice is included
  in the collapsed pet's `aria-label`/`title`.
* Edge-hidden geometry: left/right strips are `hidden_sliver × collapsed_size`;
  top/bottom strips rotate those dimensions. `collapsed_size` is the single
  along-edge size setting; changing it updates an already-hidden strip too.
  The strip stays flush with the edge and preserves its along-edge centre.
* Custom artwork: with `icon_style=custom` and an uploaded image, the collapsed
  pet (and the hidden tab, preferring the `hidden` slot) renders the image
  instead of the built-in glyph; badges stay on top. See `docs/ICONS.md`.
* Clicking a row calls `jump_to_task` once. Dragging inside the list must scroll,
  not jump, and never trigger a drag of the window. The jump ladder is verified by
  read-back: Codex L3 (deep link), DSH L3 (macOS AX sidebar click, requires
  Accessibility), VSCode L2 (project window focused with `code --reuse-window`
  when available, AX raise as fallback), app activation L1.
* A dedicated drag handle (top bar) starts window dragging; a movement threshold
  (>4 px) distinguishes drag from click.
* Hover the collapsed/edge-hidden pet → `set_expanded(true, focus=false)` after
  150 ms. Leaving the window always schedules `request_collapse` after the
  configured delay (pointer re-entering cancels it; the post-expand suppression
  window can delay but never drop the collapse).
* Clicking the pet expands it **and makes the window key** (`focus=true`). Any
  click elsewhere (other app / desktop / tray menu) then produces a window
  `blur`, which calls `request_collapse(force=true)` immediately — the popover
  behaves like a native menu-bar panel. Hover expansion deliberately never
  focuses the window, preserving the “never steal focus” rule.
* Optional “hide after snapping” (`auto_hide_after_s > 0` and `edge_hide_enabled=true`): once the pet rests
  collapsed on an edge it hides itself after that many idle seconds by calling
  `hide_pet(automatic=true)` (visible edge sliver only). Hover/drag/expand cancel
  the one-shot timer; a free-floating pet never auto-hides.
* No `focus()`, no `setInterval` faster than 30 s; relative times are updated by
  one shared 30 s timer that only rewrites time labels.
* Reduced motion: respect `prefers-reduced-motion`.
* Bilingual labels (en/zh) via a small dictionary. The language comes from the
  Settings model (`system` resolves against the OS language), never from
  localStorage after the one-shot legacy import.
* Accessibility: rows are `<button>`-like with `aria-label="Codex, Fix the parser,
  running, jump L2, click to open"`.
* Settings entry points: tray menu “Settings…” (top item), a gear button in the
  expanded footer, and Cmd+, while either app window is focused. The footer no
  longer contains a language toggle (it moved into Settings).
* Pending questions: `question_count` from the snapshot paints an amber header
  chip (`N 待回答` / `N waiting`) while expanded and switches the collapsed badge
  to the amber “question” variant; an `attention-pending` event adds a one-shot
  pulse (disabled by `animations=off` / Reduce Motion) and the collapsed pet's
  aria-label/title mention the waiting count. The panel itself is auto-expanded
  by Rust when `auto_show_on_question` is on — the page never focuses the window.
* New completions are identifiable **in the list**: each unacknowledged completion
  carries a green marker across panel sessions, until that row's jump succeeds
  or its exact conversation is manually viewed.
  `new_badge=false` hides these surfaces without acknowledging them.
* Collapsed pet status capsule (semantic, can coexist, top-left): a small
  translucent macOS-traffic-light-style vertical capsule (hairline border, inset
  highlight, soft shadow, no `backdrop-filter`) holding three 6 px glass-bead
  dots — **amber** = at least one session waits for an answer, **green** = at
  least one new completion since the last look, **red** = at least one
  failed/interrupted session. Only the amber dot gently breathes (2.6 s, scale
  1→1.06 + glow) while a question is pending; `animations=off` and Reduce Motion
  make it static. The capsule is hidden while the panel is expanded and shrinks
  to a single 3 px highest-priority dot (question > failed > new) in the 14 px
  edge/tray-hidden tab. The jump-feedback notice dot lives in the bottom-right
  corner so it never collides with the capsule. Each dot and the capsule carry
  localized `aria-label`/`title` text.
* Soft jump warnings (`degraded` / `unconfirmed`) are shown **once per task and
  reason per session**; repeated clicks on the same stale target do not re-toast.
  `failed` outcomes are always reported. This is in-memory only.
* `update-available` paints a notice (“new version available”) with an **Update**
  action that opens Settings (Updates pane); when collapsed it is the usual
  notice dot (bottom-right). No auto-download.
* `max_visible_rows` drives both the Rust window height (44 + rows×64 + 26) and
  the list's CSS max-height; `show_summary` / `show_jump_level` hide the
  corresponding row elements; `animations=off` disables window geometry
  animation and CSS transitions.
* Settings changes arrive through `settings-changed` on both pages; identical
  snapshots are still de-duplicated by render hash. No settings polling.

## Settings window (`settings.html`)

* Separate native window, label `settings`, 620×560 fixed, system title bar,
  standard close button. **Created lazily on first open** (so users who never
  open Settings pay no extra webview/RSS at idle), single instance; closing
  hides it (state preserved); the app keeps running in the menu bar and remains
  **accessory** (no Dock icon).
* Left group navigation (General / Attention / Monitoring / Desktop Pet / Jump /
  Updates / Data) and a
  scrollable content pane; the selected group is remembered in the settings
  window's own localStorage, not in `settings.json`.
* macOS Preferences behaviour: no OK/Cancel, every change is saved immediately;
  a subtle “Saved” indicator confirms writes. Reset-per-group and reset-all use
  inline double confirmation.
* Readability first: solid background (no vibrancy layer), system font stack,
  semantic colours, `light-dark()` tokens, explicit `appearance` override.
* Settings → Jump permissions: the first-run/ad-hoc guide is a **collapsed
  `<details>`** (CTA in the summary when grants are missing); an “App location”
  row shows the running bundle path so the user can verify which copy System
  Settings must contain. When Accessibility is missing a stale-grant alert
  explains that ad-hoc signatures change on rebuild. Actions: **Request
  permission** (`request_accessibility_permission`, native AX prompt), per-row
  **Open** (`open_permission_settings`), **Reveal App**, **Check again**, and an
  inline-confirmed **Reset this app's permissions…** (`reset_app_permissions`,
  this bundle id only). Returning focus to the Settings window re-runs the
  self-check, so toggling the switch in System Settings updates the pills.
* All controls are native-focusable with visible focus rings and `aria-label`s.
