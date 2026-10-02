//! A hierarchy bake's animation: every glTF animation as `<clip>.animclip`, its
//! bevy_animation_graph wrapper `<clip>.anim.ron`, and the rig's `<scene>.skn.ron`.
//!
//! Targets are keyed by the node-name path from the scene's root -- the `Name`s the hierarchy
//! writer emits ([`node_name`]) -- which is what `AnimationTargetId::from_names` binds by. The
//! glTF keyframes are written as they are: each channel keeps its own key times. `.animclip` is
//! LINEAR only, so STEP keys are doubled at each step and CUBICSPLINE keeps the value of each
//! (tangent, value, tangent) triple.
//!
//! `.animclip` (reader: `bevy_aurora::animclip`): `ANIMCLP\x01`, u32 target count, per target a
//! name path (u16 count, u16-len UTF-8 parts), u8 channel mask (T=1 R=2 S=4), then per present
//! channel in T, R, S order a u32 key count, the times, the values (T xyz, R xyzw, S xyz).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use bevy::math::{Mat4, Quat, Vec3};
use gltf::animation::{Interpolation, util::ReadOutputs};
use serde::Serialize;

use crate::gltf::node_name;

/// One channel's keys: times and `N`-wide values.
#[derive(Clone, Debug, Default)]
pub struct Track<const N: usize> {
    pub times: Vec<f32>,
    pub values: Vec<[f32; N]>,
}

/// One animated node of a clip.
#[derive(Clone, Debug, Default)]
pub struct ClipTarget {
    pub path: Vec<String>,
    pub translation: Option<Track<3>>,
    pub rotation: Option<Track<4>>,
    pub scale: Option<Track<3>>,
}

pub fn write_animclip(path: &Path, targets: &[ClipTarget]) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut w = BufWriter::new(file);
    let mut put = |b: &[u8]| w.write_all(b).map_err(|e| e.to_string());
    put(b"ANIMCLP\x01")?;
    put(&(targets.len() as u32).to_le_bytes())?;
    for t in targets {
        put(&(t.path.len() as u16).to_le_bytes())?;
        for part in &t.path {
            put(&(part.len() as u16).to_le_bytes())?;
            put(part.as_bytes())?;
        }
        let mask = t.translation.is_some() as u8
            | (t.rotation.is_some() as u8) << 1
            | (t.scale.is_some() as u8) << 2;
        put(&[mask])?;
        fn track<const N: usize>(
            put: &mut impl FnMut(&[u8]) -> Result<(), String>,
            track: &Track<N>,
        ) -> Result<(), String> {
            put(&(track.times.len() as u32).to_le_bytes())?;
            for x in &track.times {
                put(&x.to_le_bytes())?;
            }
            for v in &track.values {
                for x in v {
                    put(&x.to_le_bytes())?;
                }
            }
            Ok(())
        }
        if let Some(tr) = &t.translation {
            track(&mut put, tr)?;
        }
        if let Some(r) = &t.rotation {
            track(&mut put, r)?;
        }
        if let Some(s) = &t.scale {
            track(&mut put, s)?;
        }
    }
    Ok(())
}

// ---- bevy_animation_graph sidecars -----------------------------------------------------------
// Mirrors of the asset shapes in `bevy_animation_graph_core` (branch `aurora`), so the crate
// itself -- and bevy's renderer with it -- is not a dependency of the bake. glam serializes a
// Vec3 / Quat as a tuple, which is what these arrays write.

#[derive(Serialize)]
struct GraphTransform {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

impl GraphTransform {
    fn from_mat4(m: Mat4) -> Self {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        Self {
            translation: translation.to_array(),
            rotation: rotation.to_array(),
            scale: scale.to_array(),
        }
    }
}

#[derive(Serialize)]
struct BakedBone {
    path: Vec<String>,
    local: GraphTransform,
    character: GraphTransform,
}

#[derive(Serialize)]
enum SkeletonSource {
    Baked { root: Vec<String>, bones: Vec<BakedBone> },
}

#[derive(Serialize)]
struct SkeletonSerial {
    source: SkeletonSource,
}

#[derive(Serialize)]
enum GraphClipSource {
    AnimClip { path: String },
}

#[derive(Serialize)]
struct GraphClipSerial {
    source: GraphClipSource,
    skeleton: String,
    event_tracks: HashMap<String, ()>,
}

fn write_ron<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let cfg = ron::ser::PrettyConfig::new().struct_names(false);
    let text = ron::ser::to_string_pretty(value, cfg)
        .map_err(|e| format!("serialize {}: {e}", path.display()))?;
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

// ---- the bake ---------------------------------------------------------------------------------

/// A uniform scale baked into a node's local matrix: the translation scales, nothing else does.
fn scaled_local(node: &gltf::Node, scale: f32) -> Mat4 {
    let (t, r, s) = node.transform().decomposed();
    Mat4::from_scale_rotation_translation(Vec3::from(s), Quat::from_array(r), Vec3::from(t) * scale)
}

/// Write `doc`'s animations beside a hierarchy bake of it. `scale` is the bake's: clip
/// translations scale with the node translations they key. Returns the clips written.
pub fn bake_clips(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    out_dir: &Path,
    asset_prefix: &str,
    scene_name: &str,
    scale: f32,
) -> usize {
    if doc.animations().next().is_none() {
        return 0;
    }
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .expect("gltf has no scene");

    // Every scene node's name path and rest transforms (local, and in the scene's frame),
    // grouped by the scene root it hangs from.
    let mut paths: HashMap<usize, Vec<String>> = HashMap::new();
    let mut root_of: HashMap<usize, usize> = HashMap::new();
    let mut bones: HashMap<usize, Vec<BakedBone>> = HashMap::new();
    let mut stack: Vec<(gltf::Node, Vec<String>, Mat4, usize)> = scene
        .nodes()
        .map(|n| {
            let root = n.index();
            (n, Vec::new(), Mat4::IDENTITY, root)
        })
        .collect();
    while let Some((node, mut path, parent, root)) = stack.pop() {
        path.push(node_name(&node));
        let local = scaled_local(&node, scale);
        let character = parent * local;
        bones.entry(root).or_default().push(BakedBone {
            path: path.clone(),
            local: GraphTransform::from_mat4(local),
            character: GraphTransform::from_mat4(character),
        });
        paths.insert(node.index(), path.clone());
        root_of.insert(node.index(), root);
        for child in node.children() {
            stack.push((child, path.clone(), character, root));
        }
    }

    // The skeleton is the scene root every animated node hangs from (a rigged glTF often keeps
    // its skinned mesh as a sibling root of the armature). Animation spread over several roots
    // gets clips but no skeleton: bevy_animation_graph wants one root.
    let animated_roots: std::collections::BTreeSet<usize> = doc
        .animations()
        .flat_map(|a| a.channels().map(|c| c.target().node().index()).collect::<Vec<_>>())
        .filter_map(|n| root_of.get(&n).copied())
        .collect();
    let skeleton_asset = format!("{asset_prefix}/{scene_name}.skn.ron");
    let animated_roots: Vec<usize> = animated_roots.into_iter().collect();
    match animated_roots.as_slice() {
        &[root] => {
            let serial = SkeletonSerial {
                source: SkeletonSource::Baked {
                    root: paths[&root].clone(),
                    bones: bones.remove(&root).unwrap_or_default(),
                },
            };
            let path = out_dir.join(format!("{scene_name}.skn.ron"));
            write_ron(&path, &serial).unwrap_or_else(|e| panic!("{e}"));
        }
        roots => eprintln!(
            "animation spans {} scene roots: no skeleton written (bevy_animation_graph wants one)",
            roots.len()
        ),
    }

    let mut written = 0;
    for (i, anim) in doc.animations().enumerate() {
        let name = anim
            .name()
            .map(str::to_lowercase)
            .unwrap_or_else(|| format!("clip{i}"));
        let mut targets: HashMap<usize, ClipTarget> = HashMap::new();
        for channel in anim.channels() {
            let node = channel.target().node();
            let Some(path) = paths.get(&node.index()) else {
                continue; // animates a node outside the baked scene
            };
            let reader = channel.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let Some(times) = reader.read_inputs() else { continue };
            let times: Vec<f32> = times.collect();
            let interpolation = channel.sampler().interpolation();
            let target = targets.entry(node.index()).or_insert_with(|| ClipTarget {
                path: path.clone(),
                ..Default::default()
            });
            match reader.read_outputs() {
                Some(ReadOutputs::Translations(it)) => {
                    let values = it.map(|v| [v[0] * scale, v[1] * scale, v[2] * scale]).collect();
                    target.translation = Some(linear(&times, values, interpolation));
                }
                Some(ReadOutputs::Rotations(it)) => {
                    target.rotation = Some(linear(&times, it.into_f32().collect(), interpolation));
                }
                Some(ReadOutputs::Scales(it)) => {
                    target.scale = Some(linear(&times, it.collect(), interpolation));
                }
                _ => {} // morph weights: no deformer for them yet
            }
        }
        let mut targets: Vec<ClipTarget> = targets.into_values().collect();
        targets.sort_by(|a, b| a.path.cmp(&b.path));
        let clip = out_dir.join(format!("{name}.animclip"));
        write_animclip(&clip, &targets).unwrap_or_else(|e| panic!("{e}"));
        let wrapper = GraphClipSerial {
            source: GraphClipSource::AnimClip {
                path: format!("{asset_prefix}/{name}.animclip"),
            },
            skeleton: skeleton_asset.clone(),
            event_tracks: HashMap::new(),
        };
        write_ron(&out_dir.join(format!("{name}.anim.ron")), &wrapper)
            .unwrap_or_else(|e| panic!("{e}"));
        println!("clip {name}: {} targets -> {}", targets.len(), clip.display());
        written += 1;
    }
    written
}

/// glTF keys as LINEAR keys: STEP holds each value until the next key (a doubled key just before
/// it), CUBICSPLINE keeps each triple's value.
fn linear<const N: usize>(times: &[f32], values: Vec<[f32; N]>, mode: Interpolation) -> Track<N> {
    match mode {
        Interpolation::Linear => Track {
            times: times.to_vec(),
            values,
        },
        Interpolation::CubicSpline => Track {
            times: times.to_vec(),
            values: values.chunks(3).filter_map(|c| c.get(1).copied()).collect(),
        },
        Interpolation::Step => {
            let mut track = Track::default();
            for (k, (&t, v)) in times.iter().zip(&values).enumerate() {
                if k > 0 {
                    track.times.push(t - 1.0e-4);
                    track.values.push(values[k - 1]);
                }
                track.times.push(t);
                track.values.push(*v);
            }
            track
        }
    }
}
