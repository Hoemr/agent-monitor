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

## Commands (Tauri `invoke`)

| Command | Args | Returns | Notes |
|---|---|---|---|
| `get_snapshot` | — | Snapshot | initial paint |
| `refresh` | — | Snapshot | explicit user refresh |
| `jump_to_task` | `task_id: string` | `Result<JumpOutcome, String>` | **exactly one** jump per click; rejection only on transport failure |
| `dismiss_task` | `task_id: string` | `boolean` | remove stale/invalid record |
| `get_pet_state` | — | PetStateView | mode/edge/hidden/settings |
| `set_expanded` | `expanded: boolean, focus?: boolean` | — | resize; `focus=true` only for explicit clicks (hover expansion never focuses) |
| `request_collapse` | `force?: boolean` | — | `force=true` for explicit dismissal (outside click/blur/Escape), bypasses the post-resize suppression window |
| `open_permission_settings` | `kind: "accessibility"\|"automation"` | — | system pane |
| `get_jump_diagnostics` | — | JumpDiagnostics | permission + per-agent jump self-check (see `docs/JUMP.md`) |
| `reveal_app_in_finder` | — | Result | reveal the .app bundle so it can be added to Accessibility |
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
* **Ordering**: the snapshot is already sorted by `last_activity_secs` descending
  (newest conversation first). The UI must render in snapshot order and must not
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
* Collapsed/hidden “new completions”: `seenAtSecs` is set at boot and refreshed
  whenever the panel is expanded; the collapsed pet shows an **unfilled green
  ring** (16 px, 1 px green border, 8 % tint, green tabular number) with the count
  of `completed` tasks whose activity is newer than that, and the expanded header
  shows the same count as a chip. `new_badge=false` hides both. The edge-hidden
  tab shows a filled 6 px dot instead of a number.
* Jump notices never render inside the small pet window: the toast is painted only
  while the panel is expanded/dragging. While collapsed/edge-hidden/tray-hidden a
  text-free notice dot (top-left; amber = degraded, red = failed, blue = info) is
  shown instead, the notice lifetime is extended (~30 s), and expanding restores
  the full toast with its action button. The pending notice is included in the
  collapsed pet's `aria-label`/`title`.
* Edge-hidden geometry: the window becomes a `hidden_sliver × hidden_sliver`
  square (8–24 pt) flush with the snapped screen edge — the snapped side is
  invisible and the two perpendicular sides only expose that small square. The
  tab keeps its position along the edge, so expanding again grows from there.
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
* Optional “hide after snapping” (`auto_hide_after_s > 0`): once the pet rests
  collapsed on an edge it hides itself after that many idle seconds by calling
  the normal `hide_pet` command (edge sliver or tray). Hover/drag/expand cancel
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
* New completions are identifiable **in the list**: on opening, the ids completed
  since the last look are captured and each such row carries a green “newly
  completed” marker while the panel stays open (the header chip still shows the
  count; the collapsed green ring is unchanged).
* Collapsed pet status dots (semantic, can coexist, top-left vertical cluster):
  **yellow** = at least one session waits for an answer, **green** = at least one
  new completion since the last look, **red** = at least one failed/interrupted
  session. The cluster is hidden while the panel is expanded and uses a compact
  variant in the 14 px edge/tray-hidden tab. The jump-feedback notice dot moved to
  the bottom-right corner so it never collides with the status cluster. Each dot
  and the cluster carry localized `aria-label`/`title` text.
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
* All controls are native-focusable with visible focus rings and `aria-label`s.
