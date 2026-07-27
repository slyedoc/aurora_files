//! Round-trip check on a baked `.animclip`: parse it back into an `AnimationClip`, report
//! target/curve counts, and confirm a node's Name-PATH hashes to a key present in the clip. That
//! is the writer/reader + name-path-binding contract, checked without a GPU or a running app.
//!
//! Lives next to the writer (`solari_bsn::transcode_gltf_to_animclip`) on purpose — the binary
//! layout is defined in two places and they must agree byte-for-byte, so the check that they do
//! belongs with the half this repo owns. The reader is `bevy_animation::animclip`.
//!
//! Pass the FULL name-path, slash-separated, as it appears under the scene root — a bare node name
//! only resolves for a top-level node:
//!
//!   cargo run --release -p prop_import --bin clip_check -- assets/mech/DRV4_Drover.animclip
//!   cargo run --release -p prop_import --bin clip_check -- assets/mech/DRV4_Drover.animclip Mech_Root/Pelvis/Hip_R

use bevy::animation::animclip::parse_animclip;
use bevy::animation::AnimationTargetId;
use bevy::prelude::Name;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().cloned().expect(
        "usage: clip_check <file.animclip> [Name/Path/To/Node]",
    );
    let probe = args.get(1).cloned();

    let bytes = std::fs::read(&path).expect("read .animclip");
    let clip = parse_animclip(&bytes).expect("parse .animclip");

    let targets = clip.curves().len();
    let curves: usize = clip.curves().values().map(|v| v.len()).sum();
    println!("file:            {path}  ({} bytes)", bytes.len());
    println!("targets:         {targets}");
    println!("curves total:    {curves}");
    println!("clip duration:   {:.3} s", clip.duration());

    let Some(probe) = probe else {
        println!("(pass a slash-separated name-path to probe a specific target)");
        return;
    };
    let names: Vec<Name> = probe.split('/').map(|s| Name::new(s.to_string())).collect();
    let id = AnimationTargetId::from_names(names.iter());
    match clip.curves_for_target(id) {
        Some(cs) => println!(
            "target {probe:?}:  FOUND, {} curve(s)  (name-path binding works)",
            cs.len()
        ),
        None => println!(
            "target {probe:?}:  NOT FOUND — either that node carries no channel, or the path is \
             wrong (it must start at the scene root's direct child)"
        ),
    }
}
