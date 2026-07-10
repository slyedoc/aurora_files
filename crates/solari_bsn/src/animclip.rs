//! Transcode a glTF's rigid TRS animation into a compact `.animclip` binary — replacing the runtime
//! glTF dependency. bevy has no serialized `AnimationClip` format (only the glTF loader builds one),
//! and an `AnimationClip` is a type-erased `Box<dyn AnimationCurve>` soup that won't round-trip
//! through the `.bsn` text format; so, mirroring `ClusterMesh`'s custom-binary + loader pattern, the
//! importer packs the raw keyframe arrays here and a zero-side `AnimClipLoader` rebuilds the clip.
//!
//! An `AnimationClip` binds to entities by `AnimationTargetId` = hash of the node's NAME-PATH, so we
//! store the path (the Name chain from a top-level scene node down to the target) rather than a
//! precomputed id — the loader and the runtime wiring both hash it with bevy's `from_names`, and
//! because both derive from the same node names they agree. Keeps this writer bevy-free.
//!
//! Format (little-endian) — see the reader in zero's `bsn_anim` crate, which must match byte-for-byte:
//!   magic  "ANIMCLP\x01"                     (8 bytes)
//!   u32    target_count
//!   per target:
//!     u16  path_len;  per path component: u16 len + len UTF-8 bytes (Name chain, top-level..node)
//!     u8   channel_mask                       (bit0 = translation, bit1 = rotation, bit2 = scale)
//!     per present channel, in T,R,S order:
//!       u32  key_count
//!       key_count × f32   times (seconds)
//!       key_count × dim × f32  values         (dim: T=3, R=4 [x,y,z,w], S=3)
//!   All samplers are assumed LINEAR (Zero-Day's are); STEP/CUBICSPLINE channels are skipped + logged.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"ANIMCLP\x01";
const T: u8 = 1;
const R: u8 = 2;
const S: u8 = 4;

#[derive(Default)]
struct Channels {
    translation: Option<(Vec<f32>, Vec<f32>)>, // (times, xyz flat)
    rotation: Option<(Vec<f32>, Vec<f32>)>,    // (times, xyzw flat)
    scale: Option<(Vec<f32>, Vec<f32>)>,       // (times, xyz flat)
}

/// Read `gltf_path`'s animations and write a `.animclip` to `out_path`. Returns the target count.
pub fn transcode_gltf_to_animclip(gltf_path: &Path, out_path: &Path) -> Result<usize, String> {
    let bytes = fs::read(gltf_path).map_err(|e| format!("read {}: {e}", gltf_path.display()))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| format!("parse gltf: {e}"))?;
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, gltf_path.parent(), gltf.blob.clone())
        .map_err(|e| format!("import buffers: {e}"))?;

    // node index -> name-path (Name chain from a top-level scene node down to the node, inclusive).
    let paths = node_name_paths(doc);

    // Aggregate every animation's channels by target node (Blender emits one action per object, but
    // aggregating is robust to multi-node animations too).
    let mut targets: HashMap<usize, Channels> = HashMap::new();
    let mut skipped = 0usize;
    for anim in doc.animations() {
        for channel in anim.channels() {
            let sampler = channel.sampler();
            if sampler.interpolation() != gltf::animation::Interpolation::Linear {
                skipped += 1;
                continue;
            }
            let node = channel.target().node().index();
            let reader = channel.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let Some(times) = reader.read_inputs() else { continue };
            let times: Vec<f32> = times.collect();
            let entry = targets.entry(node).or_default();
            match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Translations(it)) => {
                    entry.translation = Some((times, it.flatten().collect()));
                }
                Some(gltf::animation::util::ReadOutputs::Rotations(rot)) => {
                    entry.rotation = Some((times, rot.into_f32().flatten().collect()));
                }
                Some(gltf::animation::util::ReadOutputs::Scales(it)) => {
                    entry.scale = Some((times, it.flatten().collect()));
                }
                _ => {} // morph weights: not a rigid TRS channel, ignore
            }
        }
    }

    // Keep only targets we have a name-path for (unnamed-ancestor nodes can't be bound by name).
    let mut records: Vec<(&Vec<String>, &Channels)> = targets
        .iter()
        .filter_map(|(node, ch)| paths.get(node).map(|p| (p, ch)))
        .collect();
    records.sort_by(|a, b| a.0.cmp(b.0)); // deterministic output

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut w = BufWriter::new(File::create(out_path).map_err(|e| format!("create: {e}"))?);
    w.write_all(MAGIC).map_err(io)?;
    wu32(&mut w, records.len() as u32)?;
    for (path, ch) in &records {
        wu16(&mut w, path.len() as u16)?;
        for name in path.iter() {
            let b = name.as_bytes();
            wu16(&mut w, b.len() as u16)?;
            w.write_all(b).map_err(io)?;
        }
        let mask = ch.translation.as_ref().map_or(0, |_| T)
            | ch.rotation.as_ref().map_or(0, |_| R)
            | ch.scale.as_ref().map_or(0, |_| S);
        w.write_all(&[mask]).map_err(io)?;
        for (present, data, dim) in [
            (mask & T != 0, &ch.translation, 3usize),
            (mask & R != 0, &ch.rotation, 4),
            (mask & S != 0, &ch.scale, 3),
        ] {
            if !present {
                continue;
            }
            let (times, values) = data.as_ref().unwrap();
            wu32(&mut w, times.len() as u32)?;
            for &t in times {
                w.write_all(&t.to_le_bytes()).map_err(io)?;
            }
            debug_assert_eq!(values.len(), times.len() * dim);
            for &v in values {
                w.write_all(&v.to_le_bytes()).map_err(io)?;
            }
        }
    }
    w.flush().map_err(io)?;
    if skipped > 0 {
        eprintln!("  animclip: skipped {skipped} non-LINEAR channels (only LINEAR supported)");
    }
    Ok(records.len())
}

/// node index -> the Name chain from a top-level scene node down to the node (inclusive). Only nodes
/// whose every ancestor is named are included (an unnamed link breaks name-path binding).
fn node_name_paths(doc: &gltf::Document) -> HashMap<usize, Vec<String>> {
    let mut out = HashMap::new();
    let scene = match doc.default_scene().or_else(|| doc.scenes().next()) {
        Some(s) => s,
        None => return out,
    };
    for node in scene.nodes() {
        walk_paths(&node, &[], &mut out);
    }
    out
}

fn walk_paths(node: &gltf::Node, prefix: &[String], out: &mut HashMap<usize, Vec<String>>) {
    let mut path = prefix.to_vec();
    match node.name() {
        Some(n) => path.push(n.to_string()),
        None => return, // unnamed link: can't build a stable name-path for this subtree
    }
    out.insert(node.index(), path.clone());
    for child in node.children() {
        walk_paths(&child, &path, out);
    }
}

fn io(e: std::io::Error) -> String {
    format!("write: {e}")
}
fn wu32<W: Write>(w: &mut W, v: u32) -> Result<(), String> {
    w.write_all(&v.to_le_bytes()).map_err(io)
}
fn wu16<W: Write>(w: &mut W, v: u16) -> Result<(), String> {
    w.write_all(&v.to_le_bytes()).map_err(io)
}
