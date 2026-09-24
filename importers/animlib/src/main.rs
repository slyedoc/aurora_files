//! Bake a skinned glb clip library onto a baked person's rig as `.animclip` files.
//!
//! The target rig (`assets/people/<name>.bsn`, Rig::GameEngine) binds with IDENTITY rotation
//! on every bone, so retargeting is a world-space delta: for each bone `j` with source
//! world rotation `S(t)` and source bind `Sb`,
//!
//!   T_world(t) = S(t) * Sb^-1 * align_j * Tb_j,   align_j = rotation(target rest bone dir -> source rest bone dir)
//!
//! where `Tb_j` is the target bone's own world REST rotation, so a source at its bind leaves the
//! target at its bind. The make_human bake is identity there and only `align_j` matters; UAL's
//! own rig is not, and it is what makes a source->itself bake come out as an identity retarget.
//!
//! and the local key is `T_parent_world(t)^-1 * T_world(t)`. `align_j` maps the target bone's
//! rest FRAME onto the source's: the bone axis (toward the primary child) plus a roll
//! reference, each rig's own facing direction (foot -> ball) projected off the axis. Aligning
//! the axis alone leaves the roll arbitrary and every joint skins twisted. Only the pelvis
//! carries translation, scaled by the pelvis-height ratio.
//! Clips are resampled at a fixed rate with linear keys.
//!
//! Format: see the reader `bevy_aurora::animclip` — `ANIMCLP\x01`, u32 target count, per
//! target a name path (u16 count, u16-len UTF-8 parts), u8 channel mask (T=1 R=2 S=4), per
//! channel u32 key count, times, values (T xyz, R xyzw, S xyz).

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use glam::{Quat, Vec3};

#[derive(Parser)]
#[command(about = "glb clip library -> .animclip files retargeted onto a baked person")]
struct Args {
    /// Source skinned glb with named animations (UAL1_Standard.glb, a Mixamo export, ...).
    source: PathBuf,
    /// Target rig: a baked person `.bsn` (bake_person output, identity bind).
    rig: PathBuf,
    /// Output directory; one `<clip>.animclip` per clip.
    out: PathBuf,
    /// Comma-separated clip names to bake (default: every clip).
    #[arg(long, value_delimiter = ',')]
    clips: Vec<String>,
    /// Resample rate.
    #[arg(long, default_value_t = 30.0)]
    fps: f32,
    /// Print the source's clip and joint names and exit.
    #[arg(long)]
    list: bool,
}

/// Source joint name -> target bone name, tried FIRST; the caller falls back to the source name
/// unchanged when the mapped one is not in the rig. The UE naming family matches except for case
/// on the root and the head, and only on the make_human bake — which is why this is a preference
/// and not a rule: baking onto UAL's OWN rig (`assets/ual/Mannequin.bsn`, a near-identity
/// retarget) wants every name verbatim. Bones the target lacks fall out of the rig lookup.
fn map_name(src: &str) -> &str {
    match src {
        "root" => "Root",
        "Head" => "head",
        s => s,
    }
}

/// End bones with no child of their own. The make_human bake has none, so their misses are
/// expected rather than something to report.
fn is_leaf_name(src: &str) -> bool {
    src.contains("_leaf")
}

/// Which child defines a bone's direction when it has several.
fn primary_child(bone: &str) -> Option<&'static str> {
    Some(match bone {
        "pelvis" => "spine_01",
        "spine_03" => "neck_01",
        "hand_l" => "middle_01_l",
        "hand_r" => "middle_01_r",
        _ => return None,
    })
}

// ---------------------------------------------------------------- target rig (.bsn)

struct TargetBone {
    parent: Option<usize>,
    local_t: Vec3,
    world_t: Vec3,
    /// Rest rotation in world space. The make_human bake is identity throughout; UAL's own rig
    /// is not, and composing its offsets without this puts the pelvis at 0.05 m instead of 0.92.
    world_r: Quat,
    children: Vec<usize>,
}

struct TargetRig {
    names: Vec<String>,
    bones: Vec<TargetBone>,
    index: HashMap<String, usize>,
}

impl TargetRig {
    /// The bake writes `Name("x")` then its `Transform { translation: ..., rotation: ... }` on
    /// the next line, nesting with 4-space indentation; that is enough to rebuild the rest pose.
    fn parse(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut names = Vec::new();
        let mut bones: Vec<TargetBone> = Vec::new();
        let mut stack: Vec<(usize, usize)> = Vec::new(); // (depth, bone)
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let depth = line.len() - line.trim_start().len();
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("bevy_ecs::name::Name(\"") {
                let name = rest.trim_end_matches("\")").to_string();
                let tf = lines.get(i + 1).copied().unwrap_or("");
                let local_t = parse_translation(tf).unwrap_or(Vec3::ZERO);
                let local_r = parse_rotation(tf).unwrap_or(Quat::IDENTITY);
                while stack.last().is_some_and(|&(d, _)| d >= depth) {
                    stack.pop();
                }
                let parent = stack.last().map(|&(_, b)| b);
                let (world_t, world_r) = match parent {
                    Some(p) => (
                        bones[p].world_t + bones[p].world_r * local_t,
                        bones[p].world_r * local_r,
                    ),
                    None => (local_t, local_r),
                };
                let idx = bones.len();
                bones.push(TargetBone {
                    parent,
                    local_t,
                    world_t,
                    world_r,
                    children: Vec::new(),
                });
                if let Some(p) = parent {
                    bones[p].children.push(idx);
                }
                names.push(name);
                stack.push((depth, idx));
            }
            i += 1;
        }
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        Ok(Self {
            names,
            bones,
            index,
        })
    }

    /// Rest rotation relative to the parent (what a `.bsn` bone line carries).
    fn local_rot(&self, b: usize) -> Quat {
        match self.bones[b].parent {
            Some(p) => self.bones[p].world_r.inverse() * self.bones[b].world_r,
            None => self.bones[b].world_r,
        }
    }

    /// World rest direction of bone `b` (toward its primary child), if it has one.
    fn bone_dir(&self, b: usize) -> Option<Vec3> {
        let name = &self.names[b];
        let child = match primary_child(name) {
            Some(c) => *self.index.get(c)?,
            None => *self.bones[b].children.first()?,
        };
        (self.bones[child].world_t - self.bones[b].world_t).try_normalize()
    }

    /// Name chain from "Armature" down to `b` (the runtime's `AnimationTargetId` path).
    fn path(&self, b: usize) -> Vec<String> {
        let mut chain = vec![];
        let mut cur = Some(b);
        while let Some(c) = cur {
            chain.push(self.names[c].clone());
            if self.names[c] == "Armature" {
                break;
            }
            cur = self.bones[c].parent;
        }
        chain.reverse();
        chain
    }
}

fn parse_translation(line: &str) -> Option<Vec3> {
    let i = line.find("translation: glam::Vec3 {")?;
    let rest = &line[i..];
    let num = |key: &str| -> Option<f32> {
        let j = rest.find(key)? + key.len();
        let tail = &rest[j..];
        let end = tail.find(|c: char| c == ',' || c == '}')?;
        tail[..end].trim().parse().ok()
    };
    Some(Vec3::new(num("x:")?, num("y:")?, num("z:")?))
}

fn parse_rotation(line: &str) -> Option<Quat> {
    let i = line.find("rotation: glam::Quat {")?;
    let rest = &line[i..];
    let num = |key: &str| -> Option<f32> {
        let j = rest.find(key)? + key.len();
        let tail = &rest[j..];
        let end = tail.find(|c: char| c == ',' || c == '}')?;
        tail[..end].trim().parse().ok()
    };
    Some(Quat::from_xyzw(
        num("x:")?,
        num("y:")?,
        num("z:")?,
        num("w:")?,
    ))
}

// ---------------------------------------------------------------- source (glb)

struct SourceNode {
    name: String,
    parent: Option<usize>,
    rest_t: Vec3,
    rest_r: Quat,
}

struct Curve {
    times: Vec<f32>,
    values: Vec<[f32; 4]>, // xyz(w)
}

impl Curve {
    fn sample_vec3(&self, t: f32) -> Vec3 {
        let (a, b, f) = self.bracket(t);
        Vec3::from_slice(&self.values[a][..3]).lerp(Vec3::from_slice(&self.values[b][..3]), f)
    }
    fn sample_quat(&self, t: f32) -> Quat {
        let (a, b, f) = self.bracket(t);
        Quat::from_array(self.values[a]).slerp(Quat::from_array(self.values[b]), f)
    }
    fn bracket(&self, t: f32) -> (usize, usize, f32) {
        let n = self.times.len();
        if n == 1 || t <= self.times[0] {
            return (0, 0, 0.0);
        }
        if t >= self.times[n - 1] {
            return (n - 1, n - 1, 0.0);
        }
        let b = self.times.partition_point(|&x| x <= t);
        let a = b - 1;
        let span = self.times[b] - self.times[a];
        let f = if span > 0.0 { (t - self.times[a]) / span } else { 0.0 };
        (a, b, f)
    }
}

#[derive(Default)]
struct NodeTracks {
    translation: Option<Curve>,
    rotation: Option<Curve>,
}

struct Clip {
    name: String,
    duration: f32,
    tracks: HashMap<usize, NodeTracks>,
}

fn load_source(path: &Path) -> Result<(Vec<SourceNode>, Vec<Clip>), String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| format!("parse gltf: {e}"))?;
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, path.parent(), gltf.blob.clone())
        .map_err(|e| format!("import buffers: {e}"))?;

    let mut nodes: Vec<SourceNode> = doc
        .nodes()
        .map(|n| {
            let (t, r, _s) = n.transform().decomposed();
            SourceNode {
                name: n.name().unwrap_or("").to_string(),
                parent: None,
                rest_t: Vec3::from(t),
                rest_r: Quat::from_array(r),
            }
        })
        .collect();
    for n in doc.nodes() {
        for c in n.children() {
            nodes[c.index()].parent = Some(n.index());
        }
    }

    let mut clips = Vec::new();
    for anim in doc.animations() {
        let mut tracks: HashMap<usize, NodeTracks> = HashMap::new();
        let mut duration: f32 = 0.0;
        for channel in anim.channels() {
            let node = channel.target().node().index();
            let reader = channel.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let Some(times) = reader.read_inputs() else { continue };
            let times: Vec<f32> = times.collect();
            duration = duration.max(times.last().copied().unwrap_or(0.0));
            let entry = tracks.entry(node).or_default();
            match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Translations(it)) => {
                    let values = it.map(|v| [v[0], v[1], v[2], 0.0]).collect();
                    entry.translation = Some(Curve { times, values });
                }
                Some(gltf::animation::util::ReadOutputs::Rotations(rot)) => {
                    let values = rot.into_f32().collect();
                    entry.rotation = Some(Curve { times, values });
                }
                _ => {}
            }
        }
        clips.push(Clip {
            name: anim.name().unwrap_or("unnamed").to_string(),
            duration,
            tracks,
        });
    }
    Ok((nodes, clips))
}

/// World rotations + positions of every source node for one pose (rest, or a clip at `t`).
fn source_world(
    nodes: &[SourceNode],
    tracks: Option<&HashMap<usize, NodeTracks>>,
    t: f32,
) -> (Vec<Quat>, Vec<Vec3>) {
    let n = nodes.len();
    let mut rot = vec![Quat::IDENTITY; n];
    let mut pos = vec![Vec3::ZERO; n];
    // glTF nodes are stored parent-before-child by convention; walk in index order with a
    // fallback pass so any order works.
    let mut done = vec![false; n];
    let mut remaining = n;
    while remaining > 0 {
        let before = remaining;
        for i in 0..n {
            if done[i] {
                continue;
            }
            let parent_ok = nodes[i].parent.is_none_or(|p| done[p]);
            if !parent_ok {
                continue;
            }
            let (mut lt, mut lr) = (nodes[i].rest_t, nodes[i].rest_r);
            if let Some(tr) = tracks.and_then(|m| m.get(&i)) {
                if let Some(c) = &tr.translation {
                    lt = c.sample_vec3(t);
                }
                if let Some(c) = &tr.rotation {
                    lr = c.sample_quat(t);
                }
            }
            match nodes[i].parent {
                Some(p) => {
                    rot[i] = rot[p] * lr;
                    pos[i] = pos[p] + rot[p] * lt;
                }
                None => {
                    rot[i] = lr;
                    pos[i] = lt;
                }
            }
            done[i] = true;
            remaining -= 1;
        }
        if remaining == before {
            break; // cycle; leave the rest at identity
        }
    }
    (rot, pos)
}

/// An orthonormal frame with `x` along `axis` and `y` toward `reference` (projected off the
/// axis; `fallback` when they're parallel, e.g. a foot bone and the facing vector).
fn frame(axis: Vec3, reference: Vec3, fallback: Vec3) -> Quat {
    let x = axis.normalize();
    let mut y = reference - x * reference.dot(x);
    if y.length_squared() < 1e-6 {
        y = fallback - x * fallback.dot(x);
    }
    let y = y.normalize();
    let z = x.cross(y);
    Quat::from_mat3(&glam::Mat3::from_cols(x, y, z))
}

/// Horizontal facing of a rig in its rest pose: foot -> ball, averaged over both feet.
fn facing(pos: impl Fn(&str) -> Option<Vec3>) -> Option<Vec3> {
    let mut acc = Vec3::ZERO;
    for (foot, ball) in [("foot_l", "ball_l"), ("foot_r", "ball_r")] {
        acc += (pos(ball)? - pos(foot)?).with_y(0.0);
    }
    acc.try_normalize()
}

// ---------------------------------------------------------------- bake

struct Target {
    path: Vec<String>,
    times: Vec<f32>,
    rotations: Vec<Quat>,
    translations: Option<Vec<Vec3>>,
}

fn write_animclip(path: &Path, targets: &[Target]) -> Result<(), String> {
    let f = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut w = BufWriter::new(f);
    let mut put = |b: &[u8]| w.write_all(b).map_err(|e| e.to_string());
    put(b"ANIMCLP\x01")?;
    put(&(targets.len() as u32).to_le_bytes())?;
    for t in targets {
        put(&(t.path.len() as u16).to_le_bytes())?;
        for part in &t.path {
            put(&(part.len() as u16).to_le_bytes())?;
            put(part.as_bytes())?;
        }
        let mask = 2u8 | if t.translations.is_some() { 1 } else { 0 };
        put(&[mask])?;
        if let Some(tr) = &t.translations {
            put(&(t.times.len() as u32).to_le_bytes())?;
            for &x in &t.times {
                put(&x.to_le_bytes())?;
            }
            for v in tr {
                for x in v.to_array() {
                    put(&x.to_le_bytes())?;
                }
            }
        }
        put(&(t.times.len() as u32).to_le_bytes())?;
        for &x in &t.times {
            put(&x.to_le_bytes())?;
        }
        for q in &t.rotations {
            for x in q.to_array() {
                put(&x.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------- bevy_animation_graph sidecars

/// Mirrors of the asset shapes in `bevy_animation_graph_core` (branch `aurora`). They exist so
/// serde writes them with the same glam impls that read them back; the crate itself is not a
/// dependency here because it would drag bevy into an importer that only needs bytes.
mod graph_assets {
    use glam::{Quat, Vec3};
    use serde::Serialize;

    #[derive(Serialize)]
    pub struct Transform {
        pub translation: Vec3,
        pub rotation: Quat,
        pub scale: Vec3,
    }

    impl Transform {
        pub fn new(translation: Vec3, rotation: Quat) -> Self {
            Self {
                translation,
                rotation,
                scale: Vec3::ONE,
            }
        }
    }

    #[derive(Serialize)]
    pub struct BakedBone {
        pub path: Vec<String>,
        pub local: Transform,
        pub character: Transform,
    }

    #[derive(Serialize)]
    pub enum SkeletonSource {
        Baked {
            root: Vec<String>,
            bones: Vec<BakedBone>,
        },
    }

    #[derive(Serialize)]
    pub struct SkeletonSerial {
        pub source: SkeletonSource,
    }

    #[derive(Serialize)]
    pub enum GraphClipSource {
        AnimClip { path: String },
    }

    #[derive(Serialize)]
    pub struct GraphClipSerial {
        pub source: GraphClipSource,
        pub skeleton: String,
        pub event_tracks: std::collections::HashMap<String, ()>,
    }
}

/// A path as the asset server sees it: whatever follows the last `assets/` component.
fn asset_path(path: &Path) -> String {
    let parts: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    match parts.iter().rposition(|p| p == "assets") {
        Some(i) => parts[i + 1..].join("/"),
        None => path.file_name().unwrap_or_default().to_string_lossy().to_string(),
    }
}

fn write_ron<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let cfg = ron::ser::PrettyConfig::new().struct_names(false);
    let text = ron::ser::to_string_pretty(value, cfg).map_err(|e| format!("serialize {}: {e}", path.display()))?;
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

/// The rig's bone tree as a `.skn.ron` beside its `.bsn`: bevy_animation_graph needs rest poses
/// in both local and character space, and on this branch it reads them baked rather than
/// walking a glTF scene at load time.
fn write_skeleton(rig: &TargetRig, rig_path: &Path) -> Result<PathBuf, String> {
    let bones: Vec<graph_assets::BakedBone> = (0..rig.bones.len())
        .map(|b| graph_assets::BakedBone {
            path: rig.path(b),
            local: graph_assets::Transform::new(rig.bones[b].local_t, rig.local_rot(b)),
            character: graph_assets::Transform::new(rig.bones[b].world_t, rig.bones[b].world_r),
        })
        .collect();
    let serial = graph_assets::SkeletonSerial {
        source: graph_assets::SkeletonSource::Baked {
            root: vec!["Armature".to_string()],
            bones,
        },
    };
    let out = rig_path.with_extension("skn.ron");
    write_ron(&out, &serial)?;
    Ok(out)
}

fn main() {
    let args = Args::parse();
    let (nodes, clips) = load_source(&args.source).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    if args.list {
        println!("clips:");
        for c in &clips {
            println!("  {}  ({:.2}s, {} nodes)", c.name, c.duration, c.tracks.len());
        }
        println!("nodes:");
        for n in &nodes {
            println!("  {}", n.name);
        }
        return;
    }
    let rig = TargetRig::parse(&args.rig).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });

    // Source joint -> target bone, plus each mapped joint's alignment and bind inverse.
    let (bind_rot, bind_pos) = source_world(&nodes, None, 0.0);
    let src_facing = facing(|n| {
        nodes
            .iter()
            .position(|x| x.name == n)
            .map(|i| bind_pos[i])
    })
    .unwrap_or(Vec3::Z);
    let dst_facing = facing(|n| rig.index.get(n).map(|&b| rig.bones[b].world_t)).unwrap_or(Vec3::Z);
    // The source's motion, turned to face the target's way: T(t) = Y S(t) Sb^-1 Fs Ft^-1.
    let yaw = Quat::from_rotation_arc(src_facing, dst_facing);
    eprintln!("facing: source {src_facing:.3}, target {dst_facing:.3}");
    struct Map {
        src: usize,
        dst: usize,
        align: Option<Quat>,
        bind_inv: Quat,
    }
    let mut maps: Vec<Map> = Vec::new();
    let mut unmapped = BTreeSet::new();
    let src_index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.as_str(), i))
        .collect();
    for (i, n) in nodes.iter().enumerate() {
        // Preferred name, else the source name verbatim (UAL's own rig).
        let mapped = map_name(&n.name);
        let Some((dst_name, &dst)) = rig
            .index
            .get_key_value(mapped)
            .or_else(|| rig.index.get_key_value(n.name.as_str()))
            .map(|(k, v)| (k.as_str(), v))
        else {
            if !n.name.is_empty() && n.name != "Armature" && !is_leaf_name(&n.name) {
                unmapped.insert(n.name.clone());
            }
            continue;
        };
        // Source rest direction toward the same primary child, in world space.
        let src_dir = {
            let child_name = match primary_child(dst_name) {
                Some(c) => Some(c.to_string()),
                None => rig.bones[dst].children.first().map(|&c| rig.names[c].clone()),
            };
            child_name
                .and_then(|c| {
                    // The child's SOURCE name: undo the two renames.
                    let src_child = match c.as_str() {
                        "Root" => "root",
                        "head" => "Head",
                        s => s,
                    };
                    src_index.get(src_child).copied()
                })
                .and_then(|c| (bind_pos[c] - bind_pos[i]).try_normalize())
        };
        // Leaves (head, fingertips) have no direction of their own: they continue their
        // parent's frame, so they inherit its alignment below.
        let align = match (rig.bone_dir(dst), src_dir) {
            (Some(td), Some(sd)) => {
                let ft = frame(td, dst_facing, Vec3::Y);
                let fs = frame(sd, src_facing, Vec3::Y);
                Some(fs * ft.inverse())
            }
            _ => None,
        };
        maps.push(Map {
            src: i,
            dst,
            align,
            bind_inv: bind_rot[i].inverse(),
        });
    }
    // Resolve leaf alignments from the nearest aligned ancestor.
    let resolved: Vec<Quat> = maps
        .iter()
        .map(|m| {
            let mut cur = Some(m.dst);
            while let Some(b) = cur {
                if let Some(a) = maps.iter().find(|x| x.dst == b).and_then(|x| x.align) {
                    return a;
                }
                cur = rig.bones[b].parent;
            }
            Quat::IDENTITY
        })
        .collect();
    let maps: Vec<Map> = maps
        .into_iter()
        .zip(resolved)
        .map(|(m, a)| Map { align: Some(a), ..m })
        .collect();
    if !unmapped.is_empty() {
        eprintln!("unmapped source joints (skipped): {}", unmapped.iter().cloned().collect::<Vec<_>>().join(", "));
    }
    let pelvis_dst = *rig.index.get("pelvis").expect("target rig has no pelvis");
    let pelvis_src = *src_index.get("pelvis").expect("source has no pelvis");
    let height_scale = rig.bones[pelvis_dst].world_t.y / bind_pos[pelvis_src].y.max(1e-4);
    eprintln!(
        "rig {}: {} bones mapped, pelvis height {:.3} vs source {:.3} (scale {:.3})",
        args.rig.display(),
        maps.len(),
        rig.bones[pelvis_dst].world_t.y,
        bind_pos[pelvis_src].y,
        height_scale
    );

    let skeleton = write_skeleton(&rig, &args.rig).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let skeleton_asset = asset_path(&skeleton);
    eprintln!("skeleton: {} bones -> {}", rig.bones.len(), skeleton.display());

    fs::create_dir_all(&args.out).expect("create out dir");
    let wanted: BTreeSet<&str> = args.clips.iter().map(|s| s.as_str()).collect();
    let dt = 1.0 / args.fps;
    let mut baked = 0;
    for clip in &clips {
        if !wanted.is_empty() && !wanted.contains(clip.name.as_str()) {
            continue;
        }
        let frames = (clip.duration / dt).round().max(1.0) as usize + 1;
        let times: Vec<f32> = (0..frames).map(|k| (k as f32 * dt).min(clip.duration)).collect();

        // Per frame: source world pose -> target world rotations -> target locals.
        let mut world_rot: Vec<Vec<Quat>> = vec![vec![Quat::IDENTITY; rig.bones.len()]; frames];
        let mut pelvis_t: Vec<Vec3> = Vec::with_capacity(frames);
        for (k, &t) in times.iter().enumerate() {
            let (rot, pos) = source_world(&nodes, Some(&clip.tracks), t);
            for m in &maps {
                // The source's world-space delta from ITS bind, re-framed, then applied to the
                // target's OWN bind. `rig.world_r` is identity throughout a make_human bake,
                // which is why this reduced to the frame correction alone for years; on UAL's
                // own rig it is not, and dropping it splays every limb.
                world_rot[k][m.dst] = yaw
                    * rot[m.src]
                    * m.bind_inv
                    * m.align.unwrap_or(Quat::IDENTITY)
                    * rig.bones[m.dst].world_r;
            }
            // The key is parent-LOCAL, so a world-space delta lands through the parent's rest.
            let delta = yaw * (pos[pelvis_src] - bind_pos[pelvis_src]) * height_scale;
            let parent_r = rig.bones[pelvis_dst]
                .parent
                .map_or(Quat::IDENTITY, |p| rig.bones[p].world_r);
            pelvis_t.push(rig.bones[pelvis_dst].local_t + parent_r.inverse() * delta);
        }
        let mut targets = Vec::new();
        for m in &maps {
            if rig.names[m.dst] == "Root" || rig.names[m.dst] == "Armature" {
                continue; // position authority is the runtime's (KCC / navmesh)
            }
            let parent = rig.bones[m.dst].parent;
            let rotations: Vec<Quat> = (0..frames)
                .map(|k| {
                    let pw = parent.map_or(Quat::IDENTITY, |p| world_rot[k][p]);
                    (pw.inverse() * world_rot[k][m.dst]).normalize()
                })
                .collect();
            targets.push(Target {
                path: rig.path(m.dst),
                times: times.clone(),
                rotations,
                translations: (m.dst == pelvis_dst).then(|| pelvis_t.clone()),
            });
        }
        let out = args.out.join(format!("{}.animclip", clip.name));
        write_animclip(&out, &targets).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
        // The graph's view of the same bytes: a ClipNode references this, not the .animclip.
        let wrapper = args.out.join(format!("{}.anim.ron", clip.name));
        write_ron(
            &wrapper,
            &graph_assets::GraphClipSerial {
                source: graph_assets::GraphClipSource::AnimClip {
                    path: asset_path(&out),
                },
                skeleton: skeleton_asset.clone(),
                event_tracks: Default::default(),
            },
        )
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
        eprintln!(
            "{}: {:.2}s, {} frames, {} targets -> {}",
            clip.name,
            clip.duration,
            frames,
            targets.len(),
            out.display()
        );
        baked += 1;
    }
    eprintln!("baked {baked} clip(s)");
}
