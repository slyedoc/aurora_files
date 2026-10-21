//! The prop sets on disk: every `.bsn` whose root carries a `bevy_aurora::prop::Prop` (the bakers
//! write it), grouped by the asset folder it sits in.
//!
//! **A bevy `AssetServer` cannot enumerate**, so this walks the filesystem: a tool runs against a
//! working tree. `bevy_aurora::prop::catalog` reads the bounds from the files' text, so the shelf
//! can lay out before a single prop has streamed.

use std::path::Path;

use bevy::prelude::*;
use bevy_aurora::prop::catalog;

/// One placeable `.bsn`, with the bounds it was baked at.
#[derive(Debug, Clone, Default)]
pub struct KitProp {
    /// The prop's name, which is its source glTF node's name sanitized.
    pub name: String,
    /// Asset-relative path to the `.bsn`, ready to hand to the `AssetServer`.
    pub bsn: String,
    /// Corners of the prop's own AABB, in metres, at its baked origin.
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl KitProp {
    pub fn min(&self) -> Vec3 {
        Vec3::from_array(self.min)
    }

    pub fn max(&self) -> Vec3 {
        Vec3::from_array(self.max)
    }

    /// The prop's extent in metres.
    pub fn size(&self) -> Vec3 {
        self.max() - self.min()
    }

    /// The uniform scale that fits this prop inside a cube of side `cell`.
    ///
    /// Capped at 1 so a teaspoon is not inflated to the size of a forge: a shelf is for finding
    /// things, and blowing every prop up to the same size throws away the one cue that tells a
    /// hand prop from a building at a glance. Small things simply sit small in their cell.
    pub fn fit(&self, cell: f32) -> f32 {
        let longest = self.size().max_element();
        if longest <= f32::EPSILON {
            1.0
        } else {
            (cell / longest).min(1.0)
        }
    }

    /// Where to put this prop's origin so it stands centred on `cell`, on the floor of the shelf.
    ///
    /// A prop bakes with its own origin wherever the artist left it — usually on the floor, but
    /// not always centred in X/Z (the blacksmith station runs from -2.5 to +3.8 across). Without
    /// this the shelf lays out by ORIGIN and the grid reads as ragged even though the cells are
    /// exact.
    pub fn shelf_offset(&self, scale: f32) -> Vec3 {
        let centre = (self.min() + self.max()) * 0.5 * scale;
        Vec3::new(-centre.x, -self.min().y * scale, -centre.z)
    }

    /// The name with its namespace prefix removed, for display only.
    ///
    /// Asset kits name things `COMP_PROP_cart_city_02`, `T_ENV_grass_city_01`, `KB3D_LNB_Lamp_A`
    /// — a run of SHOUTED segments saying which library and category a thing belongs to, followed
    /// by what it actually is. In a list where every row carries the same shout, those leading
    /// segments are the only part guaranteed to be useless, and in a narrow pane they are also
    /// the part that survives clipping while the distinguishing tail is cut off.
    ///
    /// So: drop leading segments that are entirely upper-case or digits, and stop at the first
    /// segment that is not. That is general to the convention rather than to one kit. If it would
    /// consume the whole name (a prop genuinely called `UFO`), keep the original.
    pub fn short_name(&self) -> &str {
        let mut rest = self.name.as_str();
        while let Some((head, tail)) = rest.split_once('_') {
            let shouted = !head.is_empty()
                && head
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
            if !shouted || tail.is_empty() {
                break;
            }
            rest = tail;
        }
        rest
    }
}

/// Every prop set under the asset root, by folder name.
#[derive(Resource, Default)]
pub struct Kits(pub Vec<KitEntry>);

pub struct KitEntry {
    /// The top-level asset folder, e.g. `props`, `lunarbase`.
    pub name: String,
    pub props: Vec<KitProp>,
}

/// Every prop under the asset root, grouped by its top-level folder.
pub fn discover(root: &Path) -> Kits {
    let mut kits: Vec<KitEntry> = Vec::new();
    for prop in catalog(root) {
        let folder = prop.path.split('/').next().unwrap_or_default().to_string();
        let entry = match kits.iter_mut().position(|k| k.name == folder) {
            Some(i) => &mut kits[i],
            None => {
                kits.push(KitEntry {
                    name: folder,
                    props: Vec::new(),
                });
                kits.last_mut().unwrap()
            }
        };
        entry.props.push(KitProp {
            name: prop.name,
            bsn: prop.path,
            min: prop.prop.min.to_array(),
            max: prop.prop.max.to_array(),
        });
    }
    Kits(kits)
}

#[cfg(test)]
mod tests {
    use super::KitProp;

    fn named(name: &str) -> KitProp {
        KitProp {
            name: name.into(),
            bsn: String::new(),
            min: [0.0; 3],
            max: [1.0; 3],
        }
    }

    #[test]
    fn strips_shouted_namespace_segments() {
        assert_eq!(named("COMP_PROP_cart_city_02").short_name(), "cart_city_02");
        assert_eq!(named("P_PROP_forge_city").short_name(), "forge_city");
        assert_eq!(named("T_ENV_grass_city_01").short_name(), "grass_city_01");
        // Digits count as shouting: KB3D is a library tag like any other.
        assert_eq!(named("KB3D_LNB_Lamp_A").short_name(), "Lamp_A");
    }

    #[test]
    fn keeps_names_that_are_all_namespace() {
        // Nothing would be left, so nothing is taken.
        assert_eq!(named("UFO").short_name(), "UFO");
        assert_eq!(named("P_PROP").short_name(), "PROP");
        assert_eq!(named("crate_city_01").short_name(), "crate_city_01");
    }
}
