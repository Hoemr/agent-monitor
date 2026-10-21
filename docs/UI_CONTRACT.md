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
  "attention_count": 1,
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
| `get_pet_state` | — | PetStateView | mode/edge/hidden/prefs |
| `set_expanded` | `expanded: boolean` | — | ask Rust to resize the window |
| `request_collapse` | — | — | after leave delay |
| `open_permission_settings` | `kind: "accessibility"\|"automation"` | — | system pane |
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
{ "mode": "collapsed|expanded|dragging|edge_hidden", "edge": "left|right|top|bottom|none",
  "hidden": false, "max_visible_rows": 5, "hover_expand_ms": 150, "collapse_delay_ms": 800 }
```

Events: `tasks-updated` (Snapshot), `pet-state` (PetStateView), `jump-feedback`
(JumpOutcome, used for tray or keyboard initiated jumps).

Dev-only: with `?mock=1` (or `?mock=collapsed`) the page renders a fixture and
exposes `window.__AM_TEST__` for `scripts/ui-selftest.html`. The packaged app never
loads the mock path.

## Interaction rules implemented by the frontend

* Collapsed: small round indicator (72×72) with attention badge; click expands.
* Expanded: fixed max height (5 rows visible, scroll for more) with one row per
  task showing agent name, title (ellipsis + `title` tooltip), status text **and**
  icon (not colour alone), relative last-activity time, and jump-level hint.
* Clicking a row calls `jump_to_task` once. Dragging inside the list must scroll,
  not jump, and never trigger a drag of the window.
* A dedicated drag handle (top bar) starts window dragging; a movement threshold
  (>4 px) distinguishes drag from click.
* Hover the collapsed/edge-hidden pet → `set_expanded(true)` after 150 ms.
  Leaving the window → `request_collapse` after 800 ms; pointer re-entering
  cancels it.
* No `focus()`, no `setInterval` faster than 30 s; relative times are updated by
  one shared 30 s timer that only rewrites time labels.
* Reduced motion: respect `prefers-reduced-motion`.
* Bilingual labels (en/zh) via a small dictionary; persisted in localStorage.
* Accessibility: rows are `<button>`-like with `aria-label="Codex, Fix the parser,
  running, jump L2, click to open"`.
