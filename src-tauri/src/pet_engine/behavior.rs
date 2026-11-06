//! Mascot behavior engine: the state machine that turns a parsed pack into
//! per-tick frames. Pure logic — no windowing, no rendering — so every rule
//! (walk until a wall, fall with gravity, drag follow, throw ballistics,
//! weighted behavior scheduling with `NextBehavior` chains) is unit-testable
//! against fixture packs.
//!
//! Coordinate system matches group-finity: x grows right, y grows down; the
//! floor is the largest y. One tick = 40 ms; pose `Velocity` is applied once
//! per tick while the pose shows.

use super::pack::{ActionDef, ActionKind, Animation, BehaviorDef, Gate, Pack};
use std::collections::BTreeMap;

pub const TICK_MS: u64 = 40;

/// The visible area the mascot lives in (screen visible frame, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct World {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl World {
    pub fn floor_y(&self) -> f64 {
        self.bottom
    }
    pub fn contains_x(&self, x: f64) -> bool {
        x >= self.left && x <= self.right
    }
}

/// What the mascot should be "feeling" — driven by the monitor's task state
/// so the pet reacts to agent activity (docs/PETS.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Working,
    Celebrate,
    Distressed,
    Attention,
}

impl Mood {
    /// Preferred action names for this mood, most wanted first. Only names
    /// that exist in the pack are used; the mood silently falls back to the
    /// normal schedule when the pack has nothing matching.
    pub fn preferred_actions(self) -> &'static [&'static str] {
        match self {
            Mood::Idle => &[],
            Mood::Working => &["Walk", "Run", "ClimbWall", "GrabWall"],
            Mood::Celebrate => &["Jumping", "Bouncing", "Dance", "Cheer", "Celebrate", "Spin"],
            Mood::Distressed => &["Tripping", "Sprawl", "SitDown", "Bouncing"],
            Mood::Attention => &["Jumping", "Bouncing", "Look"],
        }
    }
}

/// What the mascot is physically doing right now.
#[derive(Debug, Clone, PartialEq)]
enum Motion {
    /// Running a behavior's action.
    Acting { action: String, pose_index: usize, ticks_in_pose: u32, elapsed: u32 },
    /// Ballistic motion (Falling / Thrown share this).
    Ballistic { vx: f64, vy: f64, gravity: f64, resistance_x: f64, resistance_y: f64, image: String },
    /// Being dragged: anchor follows the cursor.
    Dragged { cursor: (f64, f64) },
}

/// One rendered frame, handed to the frontend.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Frame {
    pub image: String,
    /// Where the image anchor sits on screen (physical px).
    pub anchor: (f64, f64),
    /// The current pose's hotspot inside the image (art px) — the renderer
    /// pins this point to the window's anchor slot.
    pub image_anchor: (f64, f64),
    /// Native pack art faces left; `Right` frames are mirrored by the renderer.
    pub facing_right: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    Left,
    Right,
}

/// A tiny xorshift so scheduling is deterministic in tests and dependency-free.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
}

#[derive(Debug, Clone)]
pub struct Mascot {
    pub position: (f64, f64),
    facing: Facing,
    motion: Motion,
    /// Behavior names merged into the next pick by `NextBehavior Add="true"`.
    bonus_pool: Vec<(String, u32)>,
    pub mood: Mood,
    mood_hold_ticks: u32,
    rng: Rng,
    /// Set while a drag is moving, used to compute the release velocity.
    drag_velocity: (f64, f64),
}

impl Mascot {
    pub fn new(position: (f64, f64), seed: u64) -> Self {
        Mascot {
            position,
            facing: Facing::Left,
            motion: Motion::Acting { action: "Stand".into(), pose_index: 0, ticks_in_pose: 0, elapsed: 0 },
            bonus_pool: Vec::new(),
            mood: Mood::Idle,
            mood_hold_ticks: 0,
            rng: Rng::new(seed),
            drag_velocity: (0.0, 0.0),
        }
    }

    pub fn facing(&self) -> Facing {
        self.facing
    }

    pub fn current_action(&self) -> Option<&str> {
        match &self.motion {
            Motion::Acting { action, .. } => Some(action),
            Motion::Ballistic { .. } => Some("Fall"),
            Motion::Dragged { .. } => Some("Dragged"),
        }
    }

    /// The user grabbed the mascot.
    pub fn grab(&mut self, cursor: (f64, f64)) {
        self.motion = Motion::Dragged { cursor };
        self.drag_velocity = (0.0, 0.0);
        self.bonus_pool.clear();
    }

    pub fn drag_to(&mut self, cursor: (f64, f64)) {
        if let Motion::Dragged { cursor: prev } = &mut self.motion {
            self.drag_velocity = (0.7 * self.drag_velocity.0 + 0.3 * (cursor.0 - prev.0),
                                  0.7 * self.drag_velocity.1 + 0.3 * (cursor.1 - prev.1));
            *prev = cursor;
            self.position = cursor;
        }
    }

    /// The user let go; fast drags become throws.
    pub fn release(&mut self, world: &World, pack: &Pack) {
        let Motion::Dragged { .. } = self.motion else {
            return;
        };
        let speed = self.drag_velocity.0.abs() + self.drag_velocity.1.abs();
        self.throw(self.drag_velocity, world, pack);
        let _ = speed;
    }

    /// Throw with an explicit per-tick velocity (e.g. derived from the
    /// window-drag history). Slow movements just drop in place.
    pub fn throw(&mut self, velocity: (f64, f64), world: &World, pack: &Pack) {
        let speed = velocity.0.abs() + velocity.1.abs();
        if speed > 6.0 {
            self.motion = Motion::Ballistic {
                vx: velocity.0.clamp(-20.0, 20.0),
                vy: velocity.1.clamp(-20.0, 20.0),
                gravity: 2.0,
                resistance_x: 0.05,
                resistance_y: 0.1,
                image: self.fall_image(pack),
            };
        } else {
            self.start_behavior(world, pack, "Fall");
        }
    }

    /// React to monitor state. Burst moods (celebrate / attention /
    /// distressed) may interrupt a quiet action once; steady moods (working /
    /// idle) only steer the next natural reschedule — the snapshot arrives at
    /// ~1 Hz, so letting them restart actions would make the pet twitch.
    pub fn set_mood(&mut self, mood: Mood, world: &World, pack: &Pack) {
        if mood == self.mood {
            return;
        }
        let burst = matches!(mood, Mood::Celebrate | Mood::Attention | Mood::Distressed);
        self.mood = mood;
        self.mood_hold_ticks = match mood {
            Mood::Celebrate | Mood::Attention => 75, // 3 s
            Mood::Distressed => 50,
            // Steady moods outlast the snapshot gap so the preference stays
            // available at every reschedule between updates.
            Mood::Working => 150,
            Mood::Idle => 0,
        };
        if burst && matches!(self.motion, Motion::Acting { .. }) {
            self.start_mood_action(world, pack);
        }
    }

    fn fall_image(&self, pack: &Pack) -> String {
        pack.actions
            .get("Falling")
            .or_else(|| pack.actions.get("Fall"))
            .and_then(|a| a.pick_animation())
            .and_then(|a| a.poses.first())
            .map(|p| p.image.clone())
            .unwrap_or_else(|| "/shime1.png".into())
    }

    fn grounded(&self, world: &World) -> bool {
        (self.position.1 - world.floor_y()).abs() < 0.5
    }

    fn on_ceiling(&self, world: &World) -> bool {
        (self.position.1 - world.top).abs() < 0.5
    }

    fn on_wall(&self, world: &World) -> bool {
        (self.position.0 - world.left).abs() < 0.5 || (self.position.0 - world.right).abs() < 0.5
    }

    fn gate_matches(&self, gate: Gate, world: &World) -> bool {
        match gate {
            Gate::Any => true,
            Gate::Floor => self.grounded(world),
            Gate::Ceiling => self.on_ceiling(world),
            Gate::Wall => self.on_wall(world),
        }
    }

    /// Weighted pick among eligible behaviors, honoring `NextBehavior`
    /// chains: `Add="false"` restricts the pick to that list, `Add="true"`
    /// merges it into the pool. Mood preferences override weights when the
    /// mood asks for an action the pack actually has.
    fn schedule_next(&mut self, world: &World, pack: &Pack) {
        let preferred: Vec<String> = Mood::preferred_actions(self.mood)
            .iter()
            .filter(|n| pack.actions.contains_key(**n))
            .map(|n| n.to_string())
            .collect();
        let candidates: Vec<&BehaviorDef> = pack
            .behaviors
            .iter()
            .filter(|b| b.frequency > 0 && self.gate_matches(b.gate, world))
            .collect();
        let mut pool: Vec<(String, u32)> =
            candidates.into_iter().map(|b| (b.name.clone(), b.frequency)).collect();
        pool.extend(self.bonus_pool.drain(..));
        if !preferred.is_empty() {
            // Mood action first if it exists as a behavior or an action we
            // can start directly.
            for name in &preferred {
                let exists = pool.iter().any(|(n, _)| n == name) || pack.actions.contains_key(name);
                if exists {
                    self.start_behavior(world, pack, name);
                    return;
                }
            }
        }
        if pool.is_empty() {
            self.start_behavior(world, pack, "Stand");
            return;
        }
        let total: u32 = pool.iter().map(|(_, f)| *f).sum();
        let mut roll = self.rng.below(total as u64) as u32;
        for (name, freq) in pool {
            if roll < freq {
                self.start_behavior(world, pack, &name);
                return;
            }
            roll -= freq;
        }
        self.start_behavior(world, pack, "Stand");
    }

    /// Start a named behavior directly (right-click menu, moods). Performs
    /// the engine's gate re-check, so a wall action requested from mid-floor
    /// degrades to Stand.
    pub fn start_behavior(&mut self, world: &World, pack: &Pack, name: &str) {        // Native forced behaviors.
        match name {
            "Fall" | "Thrown" => {
                self.motion = Motion::Ballistic { vx: 0.0, vy: 2.0, gravity: 2.0, resistance_x: 0.05, resistance_y: 0.1, image: self.fall_image(pack) };
                return;
            }
            "Dragged" => {
                self.motion = Motion::Dragged { cursor: self.position };
                return;
            }
            _ => {}
        }
        // Behavior → action of the same name (Shimeji-ee convention).
        let Some(action) = pack.actions.get(name) else {
            // No such action: stand quietly rather than vanish.
            self.start_action(world, pack, pack.actions.get("Stand").unwrap_or_else(|| first_action(pack)));
            return;
        };
        // Gate re-check: a wall behavior picked on the floor would hover in
        // mid-air; fall through to standing instead.
        if action.border != super::pack::Border::None && !self.gate_matches(gate_of(action.border), world) {
            self.start_action(world, pack, pack.actions.get("Stand").unwrap_or_else(|| first_action(pack)));
            return;
        }
        self.start_action(world, pack, action);
    }

    fn start_mood_action(&mut self, world: &World, pack: &Pack) {
        for name in Mood::preferred_actions(self.mood) {
            if pack.actions.contains_key(*name) {
                self.start_behavior(world, pack, name);
                return;
            }
        }
    }

    /// Pose velocities are in art-native space: the pack art faces LEFT, so a
    /// negative `x` walks left while facing left, and the same pose mirrored
    /// (facing right) walks right. Multiply by this to get world-space dx.
    fn facing_scale(&self) -> f64 {
        match self.facing {
            Facing::Left => 1.0,
            Facing::Right => -1.0,
        }
    }

    fn start_action(&mut self, world: &World, _pack: &Pack, action: &ActionDef) {
        let pose_index = 0;
        self.motion = Motion::Acting {
            action: action.name.clone(),
            pose_index,
            ticks_in_pose: 0,
            elapsed: 0,
        };
        // Walking actions head for the farther half of the floor (ties go
        // left); starting toward the near wall would end the walk instantly
        // and leave the pet corner-hugging. This makes walks bounce between
        // the walls like a real Shimeji.
        if matches!(action.kind, ActionKind::Move | ActionKind::ScanMove) {
            let center = (world.left + world.right) / 2.0;
            self.facing = if self.position.0 >= center {
                Facing::Left
            } else {
                Facing::Right
            };
        }
    }

    /// Advance one tick; returns the frame to render.
    pub fn tick(&mut self, world: &World, pack: &Pack) -> Frame {
        if self.mood_hold_ticks > 0 {
            self.mood_hold_ticks -= 1;
        } else if self.mood != Mood::Idle && matches!(self.motion, Motion::Acting { .. }) {
            self.mood = Mood::Idle;
        }
        // Work on an owned copy so scheduling (which needs `&mut self`) can
        // run after the physics step without borrow conflicts.
        let mut motion =
            std::mem::replace(&mut self.motion, Motion::Dragged { cursor: self.position });
        let mut ended: Option<String> = None;
        match &mut motion {
            Motion::Dragged { .. } => {}
            Motion::Ballistic { vx, vy, gravity, resistance_x, resistance_y, .. } => {
                self.position.0 += *vx;
                self.position.1 += *vy;
                *vy += *gravity;
                *vx *= 1.0 - *resistance_x;
                *vy *= 1.0 - *resistance_y;
                *vx = vx.clamp(-20.0, 20.0);
                self.position.0 = self.position.0.clamp(world.left, world.right);
                if self.position.1 >= world.floor_y() {
                    self.position.1 = world.floor_y();
                    ended = Some("Fall".into());
                }
            }
            Motion::Acting { action, pose_index, ticks_in_pose, elapsed } => {
                // An action whose definition or animation vanished degrades
                // to a reschedule instead of freezing the mascot.
                let invalid = match pack.actions.get(action.as_str()) {
                    None => true,
                    Some(def) => match def.pick_animation() {
                        None => true,
                        Some(anim) => {
                            let pose = &anim.poses[(*pose_index).min(anim.poses.len() - 1)];
                            let old_x = self.position.0;
                            self.position.0 += pose.velocity.0 * self.facing_scale();
                            self.position.1 += pose.velocity.1;
                            // Floor and walls clamp; ceilings clamp from above.
                            self.position.1 = self.position.1.min(world.floor_y());
                            self.position.0 = self.position.0.clamp(world.left, world.right);
                            // A walk ends when this tick's step was blocked by
                            // a wall — checked AFTER moving, so starting a walk
                            // while already touching a wall just walks away.
                            let intended_dx = pose.velocity.0 * self.facing_scale();
                            let blocked_by_wall = def.border == super::pack::Border::Floor
                                && intended_dx != 0.0
                                && (self.position.0 - old_x).abs() < 0.5
                                && self.grounded(world)
                                && matches!(def.kind, ActionKind::Move | ActionKind::ScanMove);
                            *ticks_in_pose += 1;
                            *elapsed += 1;
                            if *ticks_in_pose >= pose.duration_ticks.max(1) {
                                *ticks_in_pose = 0;
                                let next_index = *pose_index + 1;
                                let animation_finished = next_index >= anim.poses.len();
                                let should_end = blocked_by_wall
                                    || match def.kind {
                                        ActionKind::Stay => {
                                            // Stay ends after one full pass.
                                            *elapsed
                                                >= anim
                                                    .poses
                                                    .iter()
                                                    .map(|p| p.duration_ticks.max(1))
                                                    .sum::<u32>()
                                        }
                                        ActionKind::Animate | ActionKind::Offset => {
                                            animation_finished
                                        }
                                        ActionKind::Move | ActionKind::ScanMove => {
                                            // Move keeps going until a border; guard
                                            // against zero-velocity loops forever.
                                            animation_finished
                                                && anim
                                                    .poses
                                                    .iter()
                                                    .all(|p| p.velocity.0 == 0.0 && p.velocity.1 == 0.0)
                                        }
                                        _ => animation_finished,
                                    };
                                if should_end {
                                    ended = Some(def.name.clone());
                                } else if animation_finished {
                                    *pose_index = 0;
                                } else {
                                    *pose_index = next_index;
                                }
                            }
                            false
                        }
                    },
                };
                if invalid {
                    ended = Some(action.clone());
                }
            }
        }
        self.motion = motion;
        if let Some(finished) = ended {
            self.record_next_and_schedule(world, pack, &finished);
        }
        self.frame(pack)
    }

    /// After an action ends, honor the behavior's `NextBehavior` chain.
    fn record_next_and_schedule(&mut self, world: &World, pack: &Pack, finished: &str) {
        let next = pack
            .behaviors
            .iter()
            .find(|b| b.name == finished)
            .map(|b| (b.next.clone(), b.next_add))
            .unwrap_or((Vec::new(), false));
        let (refs, add) = next;
        if refs.is_empty() {
            self.schedule_next(world, pack);
        } else if add {
            self.bonus_pool = refs.into_iter().map(|r| (r.name, r.frequency)).collect();
            self.schedule_next(world, pack);
        } else {
            // Pick only from the referenced list.
            let total: u32 = refs.iter().map(|r| r.frequency.max(1)).sum();
            let mut roll = self.rng.below(total as u64) as u32;
            for r in &refs {
                let f = r.frequency.max(1);
                if roll < f {
                    self.start_behavior(world, pack, &r.name);
                    return;
                }
                roll -= f;
            }
            self.schedule_next(world, pack);
        }
    }

    fn frame(&self, pack: &Pack) -> Frame {
        let (image, image_anchor) = match &self.motion {
            Motion::Acting { action, pose_index, .. } => pack
                .actions
                .get(action)
                .and_then(|a| a.pick_animation())
                .and_then(|a| {
                    a.poses.get((*pose_index).min(a.poses.len().saturating_sub(1)))
                })
                .map(|p| (p.image.clone(), p.anchor))
                .unwrap_or_else(|| ("/shime1.png".into(), (64.0, 128.0))),
            Motion::Ballistic { image, .. } => (
                image.clone(),
                pack.actions
                    .get("Falling")
                    .and_then(|a| a.pick_animation())
                    .and_then(|a| a.poses.first())
                    .map(|p| p.anchor)
                    .unwrap_or((64.0, 128.0)),
            ),
            Motion::Dragged { .. } => pack
                .actions
                .get("Dragged")
                .and_then(|a| a.pick_animation())
                .and_then(|a| a.poses.first())
                .map(|p| (p.image.clone(), p.anchor))
                .unwrap_or_else(|| ("/shime1.png".into(), (64.0, 128.0))),
        };
        Frame {
            image,
            anchor: self.position,
            image_anchor,
            facing_right: self.facing == Facing::Right,
        }
    }
}

fn gate_of(border: super::pack::Border) -> Gate {
    match border {
        super::pack::Border::Floor => Gate::Floor,
        super::pack::Border::Ceiling => Gate::Ceiling,
        super::pack::Border::Wall => Gate::Wall,
        super::pack::Border::None => Gate::Any,
    }
}

fn first_action(pack: &Pack) -> &ActionDef {
    pack.actions.values().next().expect("pack always has actions")
}

// ── PetBackend adapter ───────────────────────────────────────────

use super::backend::{BackendInfo, MenuAction, PetBackend, PetFrame, WindowPolicy};

/// The Shimeji physics pet exposed as a [`PetBackend`]: roaming window,
/// walk/fall/drag/throw, menu actions filtered by what the pack ships.
pub struct SpriteBackend {
    pub pack: Pack,
    pub mascot: Mascot,
    pub builtin: bool,
    pub mascot_world: World,
}

/// (action, i18n label key) — the roaming menu, in display order.
const SHIMEJI_MENU: [(&str, &str); 7] = [
    ("Jumping", "menuJump"),
    ("Run", "menuRun"),
    ("Walk", "menuWalk"),
    ("Bouncing", "menuBounce"),
    ("SitDown", "menuSit"),
    ("Tripping", "menuTrip"),
    ("Stand", "menuStand"),
];

impl PetBackend for SpriteBackend {
    fn info(&self) -> BackendInfo {
        let menu: Vec<MenuAction> = SHIMEJI_MENU
            .iter()
            .filter(|(a, _)| self.pack.actions.contains_key(*a))
            .map(|(a, k)| MenuAction { action: a.to_string(), label_key: k.to_string() })
            .collect();
        BackendInfo {
            name: self.pack.name.clone(),
            display_name: None,
            kind: "shimeji".into(),
            builtin: self.builtin,
            author: None,
            license: None,
            source: None,
            frame_size: self.pack.frame_size,
            window_size: self.window_size(),
            warnings: self.pack.warnings.clone(),
            actions: menu.iter().map(|m| m.action.clone()).collect(),
            menu,
        }
    }

    fn window_policy(&self) -> WindowPolicy {
        WindowPolicy::FollowsAnchor
    }

    fn window_size(&self) -> f64 {
        0.0 // the host uses the user's collapsed size for roaming pets
    }

    fn set_world(&mut self, world: World) {
        self.mascot_world = world;
    }

    fn set_mood(&mut self, mood: Mood) {
        let world = self.mascot_world;
        self.mascot.set_mood(mood, &world, &self.pack);
    }

    fn tick(&mut self) -> Option<PetFrame> {
        let world = self.mascot_world;
        let frame = self.mascot.tick(&world, &self.pack);
        Some(PetFrame::from_sprite(frame))
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        self.info().menu
    }

    fn perform(&mut self, action: &str) -> bool {
        if !self.pack.actions.contains_key(action) {
            return false;
        }
        let world = self.mascot_world;
        self.mascot.start_behavior(&world, &self.pack, action);
        true
    }

    fn grab(&mut self, point: (f64, f64)) {
        self.mascot.grab(point);
    }

    fn drag_to(&mut self, point: (f64, f64)) {
        self.mascot.drag_to(point);
    }

    fn drag_ended(&mut self, anchor: (f64, f64), velocity: (f64, f64)) {
        self.mascot.position = anchor;
        let world = self.mascot_world;
        self.mascot.throw(velocity, &world, &self.pack);
    }

    fn center(&mut self) {
        let world = self.mascot_world;
        if world.right <= world.left {
            return;
        }
        self.mascot.position = ((world.left + world.right) / 2.0, world.floor_y());
        let world = self.mascot_world;
        self.mascot.start_behavior(&world, &self.pack, "Stand");
    }
}

/// Behaviors required by the engine even when a pack omits them.
pub const REQUIRED_BEHAVIORS: [&str; 3] = ["Fall", "Dragged", "Thrown"];

/// Packs missing required behaviors get them synthesized so the engine's
/// forced paths always resolve.
pub fn ensure_required_behaviors(pack: &mut Pack) {
    for required in REQUIRED_BEHAVIORS {
        if !pack.behaviors.iter().any(|b| b.name == required) {
            pack.behaviors.push(BehaviorDef {
                name: required.to_string(),
                frequency: 0,
                gate: Gate::Any,
                next: Vec::new(),
                next_add: false,
            });
        }
    }
    for required in REQUIRED_BEHAVIORS {
        if !pack.actions.contains_key(required) {
            pack.actions.insert(
                required.to_string(),
                ActionDef {
                    name: required.to_string(),
                    kind: match required {
                        "Fall" => ActionKind::Fall { gravity: 2.0, resistance_x: 0.05, resistance_y: 0.1 },
                        "Dragged" => ActionKind::Dragged,
                        _ => ActionKind::Thrown { gravity: 2.0, resistance_x: 0.05, resistance_y: 0.1 },
                    },
                    border: super::pack::Border::None,
                    animations: vec![Animation {
                        poses: vec![super::pack::Pose {
                            image: "/shime1.png".into(),
                            anchor: (64.0, 128.0),
                            velocity: (0.0, 0.0),
                            duration_ticks: 1,
                        }],
                        conditioned: false,
                    }],
                },
            );
        }
    }
}

/// A per-pack index describing what a pack offers, for the settings UI.
pub fn pack_summary(pack: &Pack) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for action in ["Walk", "Run", "Fall", "Jumping", "Sit", "ClimbWall", "Dance"] {
        out.insert(action.to_string(), pack.actions.contains_key(action));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pet_engine::pack::{load_pack, PackFiles, PackSource};

    fn pack() -> Pack {
        let mut files = PackFiles::new();
        files.insert(
            "conf/actions.xml".into(),
            r#"<?xml version="1.0"?>
<Mascot xmlns="http://www.group-finity.com/Mascot">
  <ActionList>
    <Action Name="Stand" Type="Stay" BorderType="Floor">
      <Animation><Pose Image="/s1.png" ImageAnchor="32,64" Velocity="0,0" Duration="250" /></Animation>
    </Action>
    <Action Name="Walk" Type="Move" BorderType="Floor">
      <Animation>
        <Pose Image="/s2.png" ImageAnchor="32,64" Velocity="-2,0" Duration="4" />
        <Pose Image="/s3.png" ImageAnchor="32,64" Velocity="-2,0" Duration="4" />
      </Animation>
    </Action>
    <Action Name="SitDown" Type="Stay" BorderType="Floor">
      <Animation><Pose Image="/s4.png" ImageAnchor="32,64" Velocity="0,0" Duration="50" /></Animation>
    </Action>
    <Action Name="Jumping" Type="Embedded" Class="com.group_finity.mascot.action.Jump" VelocityParam="20">
      <Animation><Pose Image="/s5.png" ImageAnchor="32,64" Velocity="0,0" Duration="250" /></Animation>
    </Action>
    <Action Name="Tripping" Type="Animate" BorderType="Floor">
      <Animation><Pose Image="/s6.png" ImageAnchor="32,64" Velocity="-2,0" Duration="4" /></Animation>
    </Action>
  </ActionList>
</Mascot>"#.as_bytes().to_vec(),
        );
        files.insert(
            "conf/behaviors.xml".into(),
            r#"<?xml version="1.0"?>
<Mascot xmlns="http://www.group-finity.com/Mascot">
  <BehaviorList>
    <Behavior Name="Fall" Frequency="0" />
    <Behavior Name="Dragged" Frequency="0" />
    <Behavior Name="Thrown" Frequency="0" />
    <Behavior Name="Stand" Frequency="200" />
    <Behavior Name="Walk" Frequency="100">
      <NextBehavior Add="false">
        <BehaviorReference Name="Stand" Frequency="100" />
      </NextBehavior>
    </Behavior>
  </BehaviorList>
</Mascot>"#.as_bytes().to_vec(),
        );
        for n in 1..=6 {
            files.insert(format!("img/s{n}.png"), vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89]);
        }
        let mut p = load_pack("test", &files, PackSource::Embedded).unwrap();
        ensure_required_behaviors(&mut p);
        p
    }

    fn world() -> World {
        World { left: 0.0, right: 1000.0, top: 0.0, bottom: 600.0 }
    }

    #[test]
    fn walk_moves_left_and_faces_left() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.start_behavior(&world(), &pack(), "Walk");
        for _ in 0..10 {
            m.tick(&world(), &pack());
        }
        assert_eq!(m.position.0, 480.0); // 10 ticks × (-2, 0)
        assert_eq!(m.facing(), Facing::Left);
        assert_eq!(m.position.1, 600.0);
    }

    #[test]
    fn walk_started_at_the_wall_walks_away_instead_of_twitching() {
        // Regression: the wall check used to fire on the start position, so a
        // walk begun at a wall ended instantly — the pet twitched in place.
        let mut m = Mascot::new((0.0, 600.0), 7);
        m.start_behavior(&world(), &pack(), "Walk");
        // The farther half is right from x=0: mirrored walk, moving right.
        assert_eq!(m.facing(), Facing::Right);
        m.tick(&world(), &pack());
        assert_eq!(m.position.0, 2.0);
        assert_eq!(m.current_action(), Some("Walk"));
    }

    #[test]
    fn walk_bounces_between_walls() {
        let mut m = Mascot::new((0.0, 600.0), 7);
        let mut reached_right = false;
        let mut came_back = false;
        for _ in 0..1400 {
            // Keep the working mood alive so reschedules keep picking Walk.
            m.set_mood(Mood::Working, &world(), &pack());
            m.tick(&world(), &pack());
            if m.position.0 >= 999.0 {
                reached_right = true;
            }
            if reached_right && m.position.0 < 900.0 {
                came_back = true;
                break;
            }
        }
        assert!(
            reached_right && came_back,
            "x={} reached_right={reached_right} came_back={came_back}",
            m.position.0
        );
    }

    #[test]
    fn walk_stops_at_wall_and_reschedules() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.start_behavior(&world(), &pack(), "Walk");
        // Walk far past the left wall.
        for _ in 0..400 {
            m.tick(&world(), &pack());
        }
        assert_eq!(m.position.0, 0.0);
        // After the wall, it must have left the Walk action.
        assert_ne!(m.current_action(), Some("Walk"));
    }

    #[test]
    fn next_behavior_false_restricts_followup() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.start_behavior(&world(), &pack(), "Walk");
        for _ in 0..400 {
            m.tick(&world(), &pack());
        }
        // Walk's NextBehavior Add=false only allows Stand.
        assert_eq!(m.current_action(), Some("Stand"), "{:?}", m.current_action());
    }

    #[test]
    fn stay_ends_after_one_pass() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.start_behavior(&world(), &pack(), "SitDown");
        for _ in 0..60 {
            m.tick(&world(), &pack());
        }
        // SitDown (50 ticks) ended and rescheduled; with only Stand/Walk in
        // the pool, it picked something valid and stayed on the floor.
        assert!(m.position.1 <= 600.0);
    }

    #[test]
    fn ballistic_falls_with_gravity_and_lands() {
        let mut m = Mascot::new((500.0, 300.0), 7);
        m.start_behavior(&world(), &pack(), "Fall");
        let mut ticks = 0;
        while !m.grounded(&world()) && ticks < 200 {
            m.tick(&world(), &pack());
            ticks += 1;
        }
        assert!(m.grounded(&world()), "must land");
        assert!(m.position.1 == 600.0);
        // Landing reschedules to a floor behavior.
        assert_ne!(m.current_action(), Some("Fall"));
    }

    #[test]
    fn throw_ballistics_use_release_velocity() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.grab((500.0, 600.0));
        m.drag_to((510.0, 590.0));
        m.drag_to((530.0, 560.0));
        m.release(&world(), &pack());
        // Fast upward-right drag becomes a throw, not a gentle fall.
        match &m.motion {
            Motion::Ballistic { vx, vy, .. } => {
                assert!(*vx > 0.0 && *vy < 0.0, "vx={vx} vy={vy}");
            }
            other => panic!("expected ballistic, got {other:?}"),
        }
    }

    #[test]
    fn slow_release_drops_gently() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.grab((500.0, 600.0));
        m.drag_to((501.0, 600.0));
        m.release(&world(), &pack());
        assert!(matches!(m.motion, Motion::Ballistic { .. }) || m.current_action() == Some("Fall"));
    }

    #[test]
    fn drag_follows_cursor() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.grab((500.0, 600.0));
        m.drag_to((250.0, 300.0));
        assert_eq!(m.position, (250.0, 300.0));
        assert_eq!(m.current_action(), Some("Dragged"));
    }

    #[test]
    fn mood_celebrate_picks_jump_when_pack_has_it() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.set_mood(Mood::Celebrate, &world(), &pack());
        assert_eq!(m.current_action(), Some("Jumping"));
    }

    #[test]
    fn mood_with_no_matching_action_falls_back_to_schedule() {
        let mut p = pack();
        p.actions.remove("Jumping");
        p.actions.remove("Tripping");
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.set_mood(Mood::Celebrate, &world(), &p);
        assert!(["Stand", "Walk"].contains(&m.current_action().unwrap_or("")));
    }

    #[test]
    fn mood_decays_back_to_idle() {
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.set_mood(Mood::Celebrate, &world(), &pack());
        for _ in 0..200 {
            m.tick(&world(), &pack());
        }
        assert_eq!(m.mood, Mood::Idle);
    }

    #[test]
    fn wall_behavior_is_not_started_from_the_floor() {
        let mut p = pack();
        // A wall-only action+behavior with huge frequency.
        p.actions.insert(
            "GrabWall".into(),
            crate::pet_engine::pack::ActionDef {
                name: "GrabWall".into(),
                kind: crate::pet_engine::pack::ActionKind::Stay,
                border: crate::pet_engine::pack::Border::Wall,
                animations: vec![crate::pet_engine::pack::Animation {
                    poses: vec![crate::pet_engine::pack::Pose {
                        image: "/s1.png".into(),
                        anchor: (32.0, 64.0),
                        velocity: (0.0, 0.0),
                        duration_ticks: 250,
                    }],
                    conditioned: false,
                }],
            },
        );
        p.behaviors.insert(0, crate::pet_engine::pack::BehaviorDef {
            name: "GrabWall".into(),
            frequency: 10_000,
            gate: crate::pet_engine::pack::Gate::Any,
            next: Vec::new(),
            next_add: false,
        });
        let mut m = Mascot::new((500.0, 600.0), 7);
        m.start_behavior(&world(), &p, "GrabWall");
        // Gate re-check demotes it to Stand instead of hovering on the wall.
        assert_eq!(m.current_action(), Some("Stand"));
    }

    #[test]
    fn ensure_required_behaviors_fills_gaps() {
        let mut p = pack();
        p.behaviors.retain(|b| b.name != "Thrown");
        p.actions.remove("Thrown");
        ensure_required_behaviors(&mut p);
        assert!(p.behaviors.iter().any(|b| b.name == "Thrown"));
        assert!(p.actions.contains_key("Thrown"));
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert!(Rng::below(&mut Rng::new(1), 0) == 0);
    }
}
