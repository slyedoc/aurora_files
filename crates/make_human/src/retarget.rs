//! Source-agnostic world-rotation retargeter.
//!
//! The MakeHuman rig binds with identity world rotations (SMPL convention), so driving it
//! from any source skeleton reduces to `target_world(b) = src_world(b) * inv(src_bind(b))`
//! per mapped bone — PROVIDED both skeletons bind in the same body pose and the source
//! rotations are expressed in the target's world frame. Unmapped bones keep bind-local
//! identity and rigidly follow their nearest mapped ancestor.

use bevy::prelude::*;

/// One mapped bone: source joint index → target bone, with the slot index of the nearest
/// MAPPED ancestor in the target hierarchy (None = target root).
pub struct RetargetEntry {
    pub source: usize,
    pub bone: &'static str,
    pub parent: Option<usize>,
}

/// Bind-offset table baked from the source skeleton's bind-pose world rotations.
pub struct BakedRetarget {
    /// per entry: inverse of the source bind world rotation (target frame)
    pub bind_inv: Vec<Quat>,
}

impl BakedRetarget {
    /// `bind_world[i]` = source joint i's world rotation at bind, already in the target frame.
    pub fn new(map: &[RetargetEntry], bind_world: &[Quat]) -> Self {
        Self { bind_inv: map.iter().map(|e| bind_world[e.source].inverse()).collect() }
    }

    /// Target world rotation per map slot for one frame of source world rotations.
    pub fn world(&self, map: &[RetargetEntry], src_world: &[Quat], out: &mut [Quat]) {
        for (slot, e) in map.iter().enumerate() {
            out[slot] = src_world[e.source] * self.bind_inv[slot];
        }
    }

    /// Bone-local rotations (relative to the nearest mapped ancestor) from target worlds.
    pub fn local(map: &[RetargetEntry], world: &[Quat], slot: usize) -> Quat {
        match map[slot].parent {
            Some(p) => world[p].inverse() * world[slot],
            None => world[slot],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai::model::body::clips::{Clips, WALK};
    use ai::model::body::qpos::{
        features_to_qpos, load_tables, qpos_row_to_mujoco_bodies, QPOS_DIM,
    };
    use std::path::PathBuf;

    fn asset_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ai/motionbricks")
    }

    const MAP: &[RetargetEntry] = &[
        RetargetEntry { source: 0, bone: "pelvis", parent: None },
        RetargetEntry { source: 15, bone: "spine_01", parent: Some(0) },
        RetargetEntry { source: 3, bone: "thigh_l", parent: Some(0) },
        RetargetEntry { source: 4, bone: "calf_l", parent: Some(2) },
        RetargetEntry { source: 6, bone: "foot_l", parent: Some(3) },
        RetargetEntry { source: 9, bone: "thigh_r", parent: Some(0) },
        RetargetEntry { source: 10, bone: "calf_r", parent: Some(5) },
        RetargetEntry { source: 12, bone: "foot_r", parent: Some(6) },
        RetargetEntry { source: 18, bone: "upperarm_l", parent: Some(1) },
        RetargetEntry { source: 19, bone: "lowerarm_l", parent: Some(8) },
        RetargetEntry { source: 25, bone: "upperarm_r", parent: Some(1) },
        RetargetEntry { source: 26, bone: "lowerarm_r", parent: Some(10) },
    ];
    /// source child joint used to form each slot's bone-direction segment (-1 = skip)
    const SEGMENT_CHILD: &[i32] = &[15, -1, 4, 6, -1, 10, 12, -1, 19, 22, 26, 29];

    fn to_quat(m: &[[f32; 3]; 3]) -> Quat {
        Quat::from_mat3(&Mat3::from_cols(
            Vec3::new(m[0][0], m[1][0], m[2][0]),
            Vec3::new(m[0][1], m[1][1], m[2][1]),
            Vec3::new(m[0][2], m[1][2], m[2][2]),
        ))
    }

    /// The whole pipeline, geometrically: retargeted bone directions must track the G1's
    /// segment directions through real walk/idle poses (knees can't silently bend backward).
    #[test]
    fn retarget_tracks_source_segments() {
        let tables = load_tables(&asset_dir()).unwrap();
        let clips = Clips::load(&asset_dir()).unwrap();

        // target frame conversion: mujoco z-up → y-up, then yaw π onto the mesh's -Z forward
        let yup = Quat::from_mat3(&Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y));
        let frame = Quat::from_rotation_y(std::f32::consts::PI) * yup;

        // source bind = G1 posed to the human's T-pose: shoulder rolls out ±90°, and the
        // G1's neutral 90°-bent elbows straightened (+90 each)
        let mut bind = [0.0f32; QPOS_DIM];
        bind[3] = 1.0;
        bind[6 + 17] = std::f32::consts::FRAC_PI_2;
        bind[6 + 24] = -std::f32::consts::FRAC_PI_2;
        bind[6 + 19] = std::f32::consts::FRAC_PI_2;
        bind[6 + 26] = std::f32::consts::FRAC_PI_2;
        let (bind_pos, bind_rots) = qpos_row_to_mujoco_bodies(&bind, &tables);
        let bind_world: Vec<Quat> =
            bind_rots.iter().map(|m| frame * to_quat(m) * frame.inverse()).collect();
        let baked = BakedRetarget::new(MAP, &bind_world);

        // human bind bone directions, derived from the source bind segments (same pose)
        let seg = |pos: &[[f32; 3]; 30], a: usize, b: usize| -> Vec3 {
            let d = Vec3::new(pos[b][0] - pos[a][0], pos[b][1] - pos[a][1], pos[b][2] - pos[a][2]);
            (frame * d).normalize()
        };
        let bind_dirs: Vec<Option<Vec3>> = MAP
            .iter()
            .zip(SEGMENT_CHILD)
            .map(|(e, &c)| (c >= 0).then(|| seg(&bind_pos, e.source, c as usize)))
            .collect();

        // the human binds in a T-pose — the posed source bind must actually BE one
        // (this is what catches the G1's bent neutral elbows)
        for (slot, expect) in [
            (2, Vec3::NEG_Y),  // thigh_l down
            (5, Vec3::NEG_Y),  // thigh_r down
            (8, Vec3::NEG_X),  // upperarm_l out to the character's left
            (9, Vec3::NEG_X),  // lowerarm_l straight, same direction
            (10, Vec3::X),     // upperarm_r
            (11, Vec3::X),     // lowerarm_r
        ] {
            let d = bind_dirs[slot].unwrap();
            assert!(
                d.dot(expect) > 0.9,
                "source bind is not a T-pose at {}: {d:?} (expected ~{expect:?})",
                MAP[slot].bone
            );
        }

        // sample real poses: several frames of the walk clip
        let n = clips.num_frames[WALK];
        let feats = clips
            .motion_feature
            .index_axis(ndarray::Axis(0), WALK)
            .slice(ndarray::s![..n, ..])
            .to_owned();
        let qpos = features_to_qpos(&feats, &tables);

        let mut worst: f32 = 1.0;
        for t in (0..n).step_by(7) {
            let row: [f32; QPOS_DIM] = std::array::from_fn(|k| qpos[[t, k]]);
            let (pos, rots) = qpos_row_to_mujoco_bodies(&row, &tables);
            let src_world: Vec<Quat> =
                rots.iter().map(|m| frame * to_quat(m) * frame.inverse()).collect();
            let mut world = vec![Quat::IDENTITY; MAP.len()];
            baked.world(MAP, &src_world, &mut world);

            for (slot, (e, &c)) in MAP.iter().zip(SEGMENT_CHILD).enumerate() {
                let Some(bind_dir) = bind_dirs[slot] else { continue };
                let human_dir = world[slot] * bind_dir;
                let g1_dir = seg(&pos, e.source, c as usize);
                let align = human_dir.dot(g1_dir);
                worst = worst.min(align);
                assert!(
                    align > 0.94, // within ~20°
                    "frame {t} bone {} diverges: dot={align:.3} human={human_dir:?} g1={g1_dir:?}",
                    e.bone
                );
            }
        }
        println!("retarget segment alignment: worst dot {worst:.4}");
    }
}

