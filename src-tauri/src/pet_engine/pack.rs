//! Shimeji-ee (group-finity Mascot) pack loader.
//!
//! A pack is a directory (or an embedded set of files) with
//! `conf/actions.xml`, `conf/behaviors.xml` and `img/*.png` frames — the
//! de-facto desktop-mascot pack format, so community mascots can be dropped
//! in without conversion.
//!
//! Supported subset, kept honest in docs/PETS.md: the PascalCase
//! group-finity schema (`Action Name/Type/BorderType`, `Animation`/`Pose
//! Image/ImageAnchor/Velocity/Duration`, weighted `Behavior Name/Frequency`
//! with `NextBehavior` chains). The `#{...}` expression language is not
//! evaluated — position gates are approximated from condition keywords and
//! always re-checked against the mascot's real position by the engine, so a
//! wrong guess degrades to another valid behavior rather than a wrong pose.
//! Legacy lowercase Japanese-format packs are rejected with a clear error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One frame of one animation.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    /// Image file name as written in the pack, e.g. `/shime1.png`.
    pub image: String,
    /// Hotspot inside the image (logical px, y grows downward). The anchor
    /// point is pinned to the mascot's screen position.
    pub anchor: (f64, f64),
    /// Per-tick displacement (dx, dy) applied while this pose is showing.
    pub velocity: (f64, f64),
    /// Duration in engine ticks (1 tick = 40 ms).
    pub duration_ticks: u32,
}

/// The engine-side action classes. Embedded Java classes map onto the native
/// implementation with the same name; unknown ones degrade to `Stay` instead
/// of failing the whole pack.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind {
    /// Loop in place (Duration of one animation pass).
    Stay,
    /// Apply pose velocities; ends at a border of `border`.
    Move,
    /// Like `Move` but walks toward a random target on the floor.
    ScanMove,
    /// Play the animation once, then end.
    Animate,
    /// Native ballistic fall (Gravity/RegistanceX/RegistanceY attributes).
    Fall { gravity: f64, resistance_x: f64, resistance_y: f64 },
    /// Native jump with initial upward velocity.
    Jump { velocity: f64 },
    /// Native drag follow.
    Dragged,
    /// Native throwable (initial velocity supplied by the drag release).
    Thrown { gravity: f64, resistance_x: f64, resistance_y: f64 },
    /// Offset the anchor by the animation velocity once.
    Offset,
    /// Java classes we do not implement; plays its animation in place.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    Floor,
    Ceiling,
    Wall,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionDef {
    pub name: String,
    pub kind: ActionKind,
    pub border: Border,
    /// Animations with no `Condition` first; conditioned ones are kept last
    /// and only used when nothing unconditional exists (we do not evaluate
    /// the expression language).
    pub animations: Vec<Animation>,
}

impl ActionDef {
    /// The animation to play. Prefers unconditional ones so we never show a
    /// pose that was explicitly gated for another situation.
    pub fn pick_animation(&self) -> Option<&Animation> {
        self.animations
            .iter()
            .find(|a| !a.conditioned)
            .or_else(|| self.animations.first())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub poses: Vec<Pose>,
    /// True when the XML carried a `Condition` we did not evaluate.
    pub conditioned: bool,
}

/// Which situation a behavior is meant for, approximated from the enclosing
/// `<Condition>` keywords. The engine re-checks against the real position
/// before starting the behavior, so this is a filter, not a truth source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Any,
    Floor,
    Wall,
    Ceiling,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NextBehavior {
    pub name: String,
    pub frequency: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BehaviorDef {
    pub name: String,
    pub frequency: u32,
    pub gate: Gate,
    /// `Add="false"` (default): after this behavior ends, pick ONLY from this
    /// list. `Add="true"`: merge these into the global pool for the next pick.
    pub next: Vec<NextBehavior>,
    pub next_add: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackSource {
    Embedded,
    Directory,
}

#[derive(Debug, Clone)]
pub struct Pack {
    pub name: String,
    pub source: PackSource,
    pub actions: BTreeMap<String, ActionDef>,
    pub behaviors: Vec<BehaviorDef>,
    /// Image file name (with leading slash) → PNG bytes.
    pub images: BTreeMap<String, Vec<u8>>,
    /// Logical size of the frames (from the first decodable PNG header).
    pub frame_size: (u32, u32),
    pub warnings: Vec<String>,
}

/// Errors that should be surfaced to the user in Settings instead of silently
/// disabling the pet.
#[derive(Debug, PartialEq)]
pub enum PackError {
    MissingActions,
    MissingBehaviors,
    Parse(String),
    LegacyFormat,
    NoImages,
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::MissingActions => write!(f, "缺少 conf/actions.xml"),
            PackError::MissingBehaviors => write!(f, "缺少 conf/behaviors.xml"),
            PackError::Parse(e) => write!(f, "XML 解析失败: {e}"),
            PackError::LegacyFormat => {
                write!(f, "是旧版日文格式的宠物包，仅支持 Shimeji-ee 格式")
            }
            PackError::NoImages => write!(f, "img/ 里没有 PNG 帧"),
        }
    }
}

/// Parse a PNG's IHDR for the frame size — we never decode pixels in Rust;
/// the webview blits the bytes directly.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const IHDR_W: usize = 16;
    const IHDR_H: usize = 20;
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[IHDR_W..IHDR_W + 4].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[IHDR_H..IHDR_H + 4].try_into().ok()?);
    Some((w, h))
}

fn attr<'a>(node: roxmltree::Node<'a, 'a>, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| node.attribute(*n))
}

fn parse_pair(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn gate_from_condition(condition: &str) -> Gate {
    let c = condition.to_ascii_lowercase();
    if c.contains("ceiling") {
        Gate::Ceiling
    } else if c.contains("wall") {
        Gate::Wall
    } else if c.contains("floor") {
        Gate::Floor
    } else {
        Gate::Any
    }
}

fn embedded_fall(node: roxmltree::Node<'_, '_>) -> ActionKind {
    ActionKind::Fall {
        gravity: attr(node, &["Gravity"]).and_then(|v| v.parse().ok()).unwrap_or(2.0),
        resistance_x: attr(node, &["RegistanceX", "ResistanceX"]).and_then(|v| v.parse().ok()).unwrap_or(0.05),
        resistance_y: attr(node, &["RegistanceY", "ResistanceY"]).and_then(|v| v.parse().ok()).unwrap_or(0.1),
    }
}

fn classify_action(kind: &str, node: roxmltree::Node<'_, '_>) -> ActionKind {
    match kind {
        "Stay" => ActionKind::Stay,
        "Move" => ActionKind::Move,
        "ScanMove" => ActionKind::ScanMove,
        "Animate" => ActionKind::Animate,
        "Sequence" | "Broadcast" | "Transform" => ActionKind::Unknown,
        "Offset" => ActionKind::Offset,
        "Embedded" => {
            // Class="com.group_finity.mascot.action.Fall" → "Fall".
            let class = attr(node, &["Class"]).unwrap_or("");
            let short = class.rsplit('.').next().unwrap_or("");
            match short {
                "Fall" => embedded_fall(node),
                "Jump" => ActionKind::Jump {
                    velocity: attr(node, &["VelocityParam"])
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(20.0),
                },
                "Dragged" => ActionKind::Dragged,
                // Thrown is reached natively via a drag release; a named
                // action of this class plays ballistics from rest.
                "ThrowIE" | "Thrown" => ActionKind::Thrown { gravity: 2.0, resistance_x: 0.05, resistance_y: 0.1 },
                "Look" | "Offset" => ActionKind::Offset,
                _ => ActionKind::Unknown,
            }
        }
        _ => ActionKind::Unknown,
    }
}

fn parse_poses(animation_node: roxmltree::Node<'_, '_>) -> Animation {
    let conditioned = attr(animation_node, &["Condition"]).is_some();
    let poses = animation_node
        .children()
        .filter(|n| n.tag_name().name().eq_ignore_ascii_case("pose"))
        .map(|p| Pose {
            image: attr(p, &["Image"]).unwrap_or("/shime1.png").to_string(),
            anchor: attr(p, &["ImageAnchor"]).and_then(parse_pair).unwrap_or((64.0, 128.0)),
            velocity: attr(p, &["Velocity"]).and_then(parse_pair).unwrap_or((0.0, 0.0)),
            duration_ticks: attr(p, &["Duration"]).and_then(|v| v.parse().ok()).unwrap_or(1),
        })
        .collect();
    Animation { poses, conditioned }
}

fn parse_actions_xml(xml: &str) -> Result<BTreeMap<String, ActionDef>, PackError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| PackError::Parse(e.to_string()))?;
    let root = doc.root_element();
    // Legacy packs use lowercase tags like <action>/<act>/<pose> at the root
    // <mascot>; group-finity uses <Mascot>/<ActionList>/<Action>.
    if root.tag_name().name().eq_ignore_ascii_case("action")
        || root.tag_name().name().eq_ignore_ascii_case("actions")
    {
        return Err(PackError::LegacyFormat);
    }
    let mut actions = BTreeMap::new();
    for action in root
        .children()
        .find(|n| n.tag_name().name() == "ActionList")
        .map(|list| list.children().filter(|n| n.tag_name().name() == "Action"))
        .into_iter()
        .flatten()
    {
        let Some(name) = attr(action, &["Name"]).map(str::to_string) else {
            continue;
        };
        let kind = classify_action(attr(action, &["Type"]).unwrap_or("Embedded"), action);
        let border = match attr(action, &["BorderType"]).unwrap_or("None") {
            "Floor" => Border::Floor,
            "Ceiling" => Border::Ceiling,
            "Wall" => Border::Wall,
            _ => Border::None,
        };
        let mut animations = Vec::new();
        for anim in action.children().filter(|n| n.tag_name().name() == "Animation") {
            animations.push(parse_poses(anim));
        }
        actions.insert(name.clone(), ActionDef { name, kind, border, animations });
    }
    Ok(actions)
}

fn parse_behaviors_xml(xml: &str) -> Result<Vec<BehaviorDef>, PackError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| PackError::Parse(e.to_string()))?;
    let root = doc.root_element();
    let mut behaviors = Vec::new();
    let list = root
        .children()
        .find(|n| n.tag_name().name() == "BehaviorList")
        .ok_or_else(|| PackError::Parse("no BehaviorList".into()))?;
    for group in list.children() {
        let group_gate = attr(group, &["Condition"]).map(gate_from_condition).unwrap_or(Gate::Any);
        if group.tag_name().name() != "Behavior" && group.tag_name().name() != "Condition" {
            continue;
        }
        let behavior_nodes: Vec<_> = if group.tag_name().name() == "Behavior" {
            vec![group]
        } else {
            group.children().filter(|n| n.tag_name().name() == "Behavior").collect()
        };
        for behavior in behavior_nodes {
            let Some(name) = attr(behavior, &["Name"]).map(str::to_string) else {
                continue;
            };
            let gate = attr(behavior, &["Condition"])
                .map(gate_from_condition)
                .unwrap_or(group_gate);
            let frequency = attr(behavior, &["Frequency"]).and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut next = Vec::new();
            let mut next_add = false;
            if let Some(nb) = behavior.children().find(|n| n.tag_name().name() == "NextBehavior") {
                next_add = attr(nb, &["Add"]).map(|v| v.eq_ignore_ascii_case("true")).unwrap_or(false);
                for r in nb.children().filter(|n| n.tag_name().name() == "BehaviorReference") {
                    if let Some(rname) = attr(r, &["Name"]) {
                        next.push(NextBehavior {
                            name: rname.to_string(),
                            frequency: attr(r, &["Frequency"]).and_then(|v| v.parse().ok()).unwrap_or(1),
                        });
                    }
                }
            }
            behaviors.push(BehaviorDef { name, frequency, gate, next, next_add });
        }
    }
    Ok(behaviors)
}

/// Files of a pack keyed by their path inside the pack, e.g.
/// `conf/actions.xml`, `img/shime1.png`.
pub type PackFiles = BTreeMap<String, Vec<u8>>;

pub fn load_pack(name: &str, files: &PackFiles, source: PackSource) -> Result<Pack, PackError> {
    let actions_xml = files
        .get("conf/actions.xml")
        .ok_or(PackError::MissingActions)?;
    let behaviors_xml = files
        .get("conf/behaviors.xml")
        .ok_or(PackError::MissingBehaviors)?;
    let actions = parse_actions_xml(&String::from_utf8_lossy(actions_xml))?;
    let behaviors = parse_behaviors_xml(&String::from_utf8_lossy(behaviors_xml))?;

    let mut images = BTreeMap::new();
    let mut frame_size = (0u32, 0u32);
    let mut warnings = Vec::new();
    for (path, bytes) in files {
        let Some(stripped_path) = path.strip_prefix("img/") else {
            continue;
        };
        if !stripped_path.ends_with(".png") && !stripped_path.ends_with(".PNG") {
            continue;
        }
        let key = format!("/{stripped_path}");
        if frame_size.0 == 0 {
            if let Some(size) = png_size(bytes) {
                frame_size = size;
            }
        }
        images.insert(key, bytes.clone());
    }
    if images.is_empty() {
        return Err(PackError::NoImages);
    }
    // Reference frames the animations name but the pack does not ship.
    for action in actions.values() {
        for anim in &action.animations {
            for pose in &anim.poses {
                if !images.contains_key(&pose.image) {
                    warnings.push(format!("{} 引用了缺失的帧 {}", action.name, pose.image));
                }
            }
        }
    }
    if behaviors.is_empty() {
        warnings.push("behaviors.xml 没有可用的行为".into());
    }
    Ok(Pack {
        name: name.to_string(),
        source,
        actions,
        behaviors,
        images,
        frame_size,
        warnings,
    })
}

/// Read a pack from a directory on disk (`conf/` + `img/`).
pub fn load_pack_dir(dir: &Path) -> Result<Pack, PackError> {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "pack".into());
    let mut files: PackFiles = BTreeMap::new();
    collect_files(dir, "", &mut files)?;
    load_pack(&name, &files, PackSource::Directory)
}

fn collect_files(dir: &Path, prefix: &str, out: &mut PackFiles) -> Result<(), PackError> {
    let entries = std::fs::read_dir(dir).map_err(|_| PackError::MissingActions)?;
    for entry in entries.flatten() {
        let path = entry.path();
        let key = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if path.is_dir() {
            collect_files(&path, &format!("{key}/"), out)?;
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.insert(key, bytes);
        }
    }
    Ok(())
}

/// The directory custom packs live in.
pub fn pets_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("agent-monitor/pets"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIONS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<Mascot xmlns="http://www.group-finity.com/Mascot">
    <ActionList>
        <Action Name="Stand" Type="Stay" BorderType="Floor">
            <Animation>
                <Pose Image="/shime1.png" ImageAnchor="32,64" Velocity="0,0" Duration="250" />
            </Animation>
        </Action>
        <Action Name="Walk" Type="Move" BorderType="Floor">
            <Animation Condition="#{mascot.lookRight}">
                <Pose Image="/shime1.png" ImageAnchor="32,64" Velocity="2,0" Duration="6" />
            </Animation>
            <Animation>
                <Pose Image="/shime2.png" ImageAnchor="32,64" Velocity="-2,0" Duration="6" />
                <Pose Image="/shime3.png" ImageAnchor="32,64" Velocity="-2,0" Duration="6" />
            </Animation>
        </Action>
        <Action Name="Falling" Type="Embedded" Class="com.group_finity.mascot.action.Fall"
            RegistanceX="0.05" RegistanceY="0.1" Gravity="2">
            <Animation>
                <Pose Image="/shime4.png" ImageAnchor="32,64" Velocity="0,0" Duration="250" />
            </Animation>
        </Action>
        <Action Name="Jumping" Type="Embedded" Class="com.group_finity.mascot.action.Jump" VelocityParam="20">
            <Animation>
                <Pose Image="/shime5.png" ImageAnchor="32,64" Velocity="0,0" Duration="250" />
            </Animation>
        </Action>
    </ActionList>
</Mascot>"##;

    const BEHAVIORS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<Mascot xmlns="http://www.group-finity.com/Mascot">
    <BehaviorList>
        <Behavior Name="Fall" Frequency="0" />
        <Behavior Name="Dragged" Frequency="0" />
        <Behavior Name="Thrown" Frequency="0" />
        <Condition Condition="#{mascot.environment.floor.isOn(mascot.anchor)}">
            <Behavior Name="Stand" Frequency="200" />
            <Behavior Name="Walk" Frequency="100">
                <NextBehavior Add="false">
                    <BehaviorReference Name="Stand" Frequency="100" />
                </NextBehavior>
            </Behavior>
        </Condition>
    </BehaviorList>
</Mascot>"##;

    fn fixture_files() -> PackFiles {
        let mut files = PackFiles::new();
        files.insert("conf/actions.xml".into(), ACTIONS.as_bytes().to_vec());
        files.insert("conf/behaviors.xml".into(), BEHAVIORS.as_bytes().to_vec());
        // Minimal 1×1 PNG (IHDR says 1×1).
        files.insert(
            "img/shime1.png".into(),
            vec![
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48, 0x44,
                0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89,
            ],
        );
        for n in 2..=5 {
            files.insert(format!("img/shime{n}.png"), files["img/shime1.png"].clone());
        }
        files
    }

    #[test]
    fn png_size_reads_ihdr() {
        assert_eq!(png_size(&fixture_files()["img/shime1.png"]), Some((1, 1)));
        assert_eq!(png_size(b"not a png"), None);
    }

    #[test]
    fn parses_group_finity_actions() {
        let pack = load_pack("test", &fixture_files(), PackSource::Embedded).unwrap();
        assert_eq!(pack.frame_size, (1, 1));
        let walk = &pack.actions["Walk"];
        assert_eq!(walk.kind, ActionKind::Move);
        assert_eq!(walk.border, Border::Floor);
        // Two animations: the conditioned one must NOT be picked first.
        assert_eq!(walk.animations.len(), 2);
        let picked = walk.pick_animation().unwrap();
        assert!(!picked.conditioned);
        assert_eq!(picked.poses.len(), 2);
        assert_eq!(picked.poses[0].velocity, (-2.0, 0.0));
        assert_eq!(picked.poses[0].duration_ticks, 6);

        let fall = &pack.actions["Falling"];
        assert_eq!(
            fall.kind,
            ActionKind::Fall { gravity: 2.0, resistance_x: 0.05, resistance_y: 0.1 }
        );
        assert_eq!(pack.actions["Jumping"].kind, ActionKind::Jump { velocity: 20.0 });
    }

    #[test]
    fn parses_behaviors_with_gates_and_next_chains() {
        let pack = load_pack("test", &fixture_files(), PackSource::Embedded).unwrap();
        let walk = pack.behaviors.iter().find(|b| b.name == "Walk").unwrap();
        assert_eq!(walk.frequency, 100);
        assert_eq!(walk.gate, Gate::Floor);
        assert!(!walk.next_add);
        assert_eq!(walk.next, vec![NextBehavior { name: "Stand".into(), frequency: 100 }]);
        // Required special behaviors are present even with zero frequency.
        for required in ["Fall", "Dragged", "Thrown"] {
            assert!(pack.behaviors.iter().any(|b| b.name == required), "{required}");
        }
    }

    #[test]
    fn missing_pieces_are_reported_not_panicked() {
        let mut files = fixture_files();
        files.remove("conf/behaviors.xml");
        assert!(matches!(
            load_pack("t", &files, PackSource::Embedded),
            Err(PackError::MissingBehaviors)
        ));

        let mut files = fixture_files();
        files.retain(|k, _| k.starts_with("conf/"));
        assert!(matches!(load_pack("t", &files, PackSource::Embedded), Err(PackError::NoImages)));
    }

    #[test]
    fn legacy_lowercase_format_is_rejected_clearly() {
        let mut files = PackFiles::new();
        files.insert(
            "conf/actions.xml".into(),
            "<?xml version=\"1.0\"?><action name=\"Stand\" type=\"動作\"><act type=\"動作\" border=\"0\"><pose x=\"0\" y=\"0\" frame=\"0\" interval=\"2\"/></act></action>".as_bytes().to_vec(),
        );
        files.insert("conf/behaviors.xml".into(), BEHAVIORS.as_bytes().to_vec());
        files.insert("img/shime1.png".into(), fixture_files()["img/shime1.png"].clone());
        assert!(matches!(
            load_pack("t", &files, PackSource::Embedded),
            Err(PackError::LegacyFormat)
        ));
    }

    #[test]
    fn missing_frame_references_become_warnings_not_errors() {
        let mut files = fixture_files();
        files.remove("img/shime5.png");
        files.remove("/img/shime5.png");
        let pack = load_pack("t", &files, PackSource::Embedded).unwrap();
        assert!(pack.warnings.iter().any(|w| w.contains("shime5.png")), "{:?}", pack.warnings);
    }
}
