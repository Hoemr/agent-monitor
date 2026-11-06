# Pet system (Rust pet engine)

The Desktop Pet has two presentations, selected in **Settings → Desktop Pet →
Pet style**:

| Style | What it is |
|---|---|
| **Capsule** (default) | The classic presentation: capsule, status beads, badge — unchanged behaviour. |
| **Mascot** | A living pet driven by the Rust pet engine. Two kinds ship today: roaming Shimeji sprites (walk / fall / drag / throw physics) and Familiar-format state-image pets (expressive states that follow the monitor's moods). |

## Architecture — one backend contract, many pet kinds

```
src-tauri/src/pet_engine/
  backend.rs   ★ PetBackend trait — the single seam between the host and any
               pet implementation: info / window policy / tick frames /
               mood / right-click menu / drag lifecycle.
  pack.rs      Shimeji-ee (group-finity) pack loader.
  behavior.rs  Shimeji state machine + SpriteBackend adapter.
  state.rs     Familiar-format state packs + StateBackend.
  mascot.rs    Host: backend registry, window policy application, drag
               trajectory → throw, pack discovery (built-ins + pets/).
```

**Extensibility contract:** a new pet kind (Live2D, GIF, anything) implements
`PetBackend` and registers a loader in `mascot.rs::build_backend`. The host
never branches on the concrete kind; the right-click menu is backend-defined
(`BackendInfo.menu`: action id + i18n key, rendered as a NATIVE popup), and
the frontend renders whatever `pet-frame` payloads the backend emits
(`sprite` frames or `state` images).

**"The pet is just different artwork":** every pet kind uses the CLASSIC
geometry pipeline — auto-snap to edges, edge-hide to a sliver (a piece of
the pet peeks out), the three status dots (amber/green/red), auto-hide after
snapping, `collapsed_size`. The only roaming exception is the Shimeji
backend, which declares `WindowPolicy::FollowsAnchor` and throws on drag
release; state pets declare `Fixed` and never move on their own.

Mood mapping per backend: the Shimeji backend biases behavior choice
(`Walk`/`Jumping`/…), the state backend maps mood → state image
(idle/working/watching/celebrating/alarmed) with `idle` fallback. Task moods stay synchronized with the same snapshot as the task panel; steady
moods never restart animations.

## Architecture

```
src-tauri/src/pet_engine/
  pack.rs      Shimeji-ee (group-finity) pack loader: actions.xml, behaviors.xml,
               img/*.png. Pure parser, unit-tested against fixture packs.
  behavior.rs  The mascot state machine: walk / stay / sit / fall / drag /
               throw, weighted behavior scheduling with NextBehavior chains,
               mood preferences. One tick = 40 ms, coordinates match
               group-finity (y grows down, floor = bottom).
  mascot.rs    Host: built-in packs embedded via include_bytes!, on-disk pack
               cache, import validation, anchor⇄window geometry, drag
               trajectory → throw velocity.
```

* A ticker thread steps the engine every 40 ms and moves the pet window so the
  mascot's anchor lands at the right screen point, then emits `pet-frame`
  (`image`, `anchor`, `image_anchor`, `facing_right`).
* Pose velocities are **art-native** (the art faces left); the engine mirrors
  both the sprite and the velocity when facing right. Walks head for the
  farther half of the floor, so they bounce between the walls like a real
  Shimeji instead of corner-hugging.
* The webview is a thin blit layer: it loads frame PNGs through
  `get_pet_pack_image` (cached data URLs), mirrors the art when facing right,
  and hides the capsule chrome (`mode-mascot`).
* **The roaming sprite sits on a fully transparent window**: while the mascot
  shows, the window vibrancy is cleared (`window_vibrancy::clear_vibrancy`
  — tauri's `set_effects(None)` is a Windows-only no-op on macOS) and the
  capsule background is removed via CSS; both are restored whenever the
  classic capsule or the expanded panel shows. The objc calls are dispatched
  to the main thread — calling them from the ticker thread aborts.
* Dragging uses the normal native window drag. On release the recorded window
  trajectory becomes a throw velocity (fast fling → ballistic arc, slow move →
  drop in place).
* **The mascot expands by left-click, not hover** (hover stays with the
  classic capsule).
* **Right-click action menu**: right-clicking the mascot pops a NATIVE menu
  at the cursor (`pet_menu_open` → `Window::popup_menu_at`). Items come from
  the backend (`BackendInfo.menu`), localized in Rust from the language
  setting, so menus never touch window geometry — the pet cannot shift,
  snap-break or clip because of a menu. Selections arrive via
  `app.on_menu_event` (`pet-action:<id>` → `backend.perform`).
* Mood updates arrive at ~1 Hz from the same task snapshot as the panel,
  even while the panel is expanded or the pet is hidden. Waiting for input or
  approval keeps the pet watching; a recent actionable failure keeps it alarmed;
  otherwise running tasks keep it working. Deliberate cancellations and expired
  failures follow the panel's attention rules and do not raise an alarm.
* Each new completion identity can celebrate for 3 seconds (pending questions
  and actionable failures take priority), then resume any remaining running tasks. Existing history
  at startup and repeated scans do not replay celebrations. A second completed
  turn in the same conversation can celebrate again without a 30-minute cooldown.
* The state-image pet goes idle when nothing needs attention, briefly looks
  around at 10 seconds and daydreams at 30 seconds, then sleeps after 60 seconds.
  Only available artwork is used; missing quiet poses fall back to idle.
  New work or questions interrupt these quiet activities immediately. Manual interactions return to the live mood
  when they finish; questions and failures interrupt them on the next mood update.
* **Expanded-panel dismissal fallback**: the panel closes on window blur;
  as a belt-and-braces path, the ticker also collapses an unfocused panel
  whose cursor has been parked elsewhere for ~600 ms (via
  `CGEventGetLocation`, which needs no accessibility permission). This
  catches the case where the focus was never acquired or the blur event was
  lost; hover-expanded panels keep the pointer inside and still close via
  the mouse-leave path.

## Visibility and interactions

Automatic hiding only runs when **Hide to edge** is enabled: it always leaves
an edge strip. With that option off the pet stays visible until explicitly
hidden. The menu-bar **Show Pet** and **Reset Position** remain available.
Reset also resets the roaming engine and republishes the visible state.

Right-click the collapsed pet or its edge strip for **Show Tasks**,
**Keep Pet Visible** (turns auto-hide off), **Reset Position**, **Settings**,
and an explicit **Hide Pet**. State packs expose only interactions with
matching artwork: head pat, celebrate, nap, look around, think, work together,
and surprise. **Follow task status** cancels the interaction immediately.
Actions appear on the next engine tick; repeated actions restart their hold.

## Packs

The picker includes these state-image packs, embedded in the binary:

| Pack | Frames | Notes |
|---|---|---|
| `nailong` (奶龙) | 8 × 512 px APNG | Golden baby dragon, detailed pixel-art shading |
| `doraemon` (哆啦 A 梦) | 8 × 512 px APNG | Blue robot cat, crisp pixel outlines and color clusters |
| `tihun` (啼魂) | 8 × 512 px APNG | Animation-inspired green monkey, tuft-shaped pixel fur |
| `british-blue` (皮蛋) | 8 × 1024 px APNG | Existing Familiar cat artwork |

The new characters share idle / working / thinking / interacting / celebrating /
alarmed / sleeping / watching states, with subtle two-frame breathing. The new sprites are composed on a 128-pixel
grid, enlarged 4× without interpolation, and displayed with crisp pixel scaling
at the user's selected pet size. Settings shows
localized pack names and idle thumbnails. Existing `douhua-cat` and
`huajuan-cat` installations remain discoverable under the user data directory.

Artwork prompts and source sheets: `output/pet-art/`. Slice and normalize each
sheet with `swift scripts/prepare-character-pets.swift <sheet> <pack-directory>`.
See [ASSETS.md](ASSETS.md) for provenance.

The retired `agent-cat` and `byte-slime` packs remain loadable for compatibility
and engine fixtures, but no longer appear in the picker. Saved choices migrate
to `nailong` and `doraemon` respectively; other selected packs are preserved.
Their old pixel-art generator remains `scripts/make-pets.py`.

### Importing custom packs (Shimeji-ee compatible)

Settings → Desktop Pet → **Import pack…** picks a Shimeji-ee pack folder
(`conf/actions.xml`, `conf/behaviors.xml`, `img/shime*.png`). The folder is
validated **in memory first** — nothing is written unless it parses; broken
packs surface their reason in the picker. Imported packs live in
`~/Library/Application Support/agent-monitor/pets/<name>/`.

Community mascots in the modern Shimeji-ee format work directly
([shimejis.xyz](https://shimejis.xyz) and similar). Legacy lowercase
Japanese-format packs are rejected with a clear error.

### Supported subset (honest scope)

* Action types: `Stay`, `Move`, `ScanMove`, `Animate`, `Offset`, and the
  embedded `Fall` / `Jump` / `Dragged` classes. `Sequence`, `Broadcast`,
  `Transform` and unknown Java classes degrade to playing the animation in
  place — never a crash, never an empty window.
* Animations: poses with `Image` / `ImageAnchor` / `Velocity` / `Duration`.
  Multiple `Animation` blocks: unconditional ones are preferred; `Condition`
  expressions (`#{…}`) are **not evaluated** — position gates are approximated
  from condition keywords and always re-checked against the mascot's real
  position, so a wrong guess degrades to another valid behavior.
* Behaviors: weighted `Frequency` pick + `NextBehavior` chains
  (`Add="false"` restricts the follow-up, `Add="true"` merges into the pool).
* Missing `Fall` / `Dragged` / `Thrown` behaviors are synthesized so the
  engine's forced paths always resolve; missing frames become warnings, not
  errors.
* Single-screen v1: the mascot roams within its current screen's visible
  frame; crossing displays is a follow-up.

## Settings keys

| Key | Values | Default |
|---|---|---|
| `pet_kind` | `classic` \| `mascot` | `classic` |
| `mascot_pack` | built-in name or `pets/` directory name | `nailong` |

If the selected pack fails to load at runtime, the pet falls back to the
classic capsule and `pet_kind` is reset — an invisible pet is never shown.
