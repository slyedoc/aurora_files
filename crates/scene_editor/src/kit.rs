//! Kit manifests: what a baked prop set contains, and how to find every set on disk.
//!
//! **A bevy `AssetServer` cannot enumerate.** There is no "list the `.bsn` files under this
//! folder" call, and a shipped build has no folder to walk anyway — the asset source may be a
//! processed cache or a remote reader. So a palette cannot discover its own contents at runtime;
//! something has to have written the list down. `prop_import` does, at bake time, because that
//! is the one moment the answer is known for free.
//!
//! That leaves the same problem one level up — which KITS exist — and it gets the opposite
//! answer: a tool may walk the filesystem, because a tool runs against a working tree. So
//! discovery scans for `*.kit.ron`, and everything below that point is data.

use std::path::{Path, PathBuf};

use bevy::{asset::AssetPath, prelude::*};
use serde::Deserialize;

/// One baked prop set: the manifest `prop_import` writes beside the `.bsn` files it produced.
#[derive(Asset, TypePath, Debug, Deserialize)]
pub struct Kit {
    pub props: Vec<KitProp>,
}

/// One placeable `.bsn`, with the bounds it was baked at.
#[derive(Debug, Clone, Deserialize)]
pub struct KitProp {
    /// The prop's name, which is its source glTF node's name sanitized.
    pub name: String,
    /// Asset-relative path to the `.bsn`, ready to hand to the `AssetServer`.
    pub bsn: String,
    /// Corners of the prop's own AABB, in metres, at its baked origin. Stored as arrays rather
    /// than `Vec3` because RON writes a 3-tuple and glam's serde impl expects a struct.
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

/// Every kit found under the asset root, in load order.
#[derive(Resource, Default)]
pub struct Kits(pub Vec<KitEntry>);

pub struct KitEntry {
    /// The manifest's own stem, e.g. `fantasy_props`.
    pub name: String,
    pub handle: Handle<Kit>,
}

/// Reads `<name>.kit.ron`.
#[derive(Default, TypePath)]
pub struct KitLoader;

impl bevy::asset::AssetLoader for KitLoader {
    type Asset = Kit;
    type Settings = ();
    type Error = KitLoadError;

    fn extensions(&self) -> &[&str] {
        // The FULL extension, not `ron`. Bevy resolves a loader by trying the whole extension
        // chain first (`get_full_extension` -> "kit.ron") and only then the secondary ones, so
        // claiming the compound extension takes precedence over any plain `.ron` loader in the
        // app without either having to know about the other.
        &["kit.ron"]
    }

    async fn load(
        &self,
        reader: &mut dyn bevy::asset::io::Reader,
        _: &Self::Settings,
        _: &mut bevy::asset::LoadContext<'_>,
    ) -> Result<Kit, KitLoadError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ron::de::from_bytes(&bytes)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KitLoadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("ron: {0}")]
    Ron(#[from] ron::error::SpannedError),
}

/// Find every `*.kit.ron` under the asset root and start it loading.
///
/// Walks the filesystem rather than asking the asset server, for the reason in the module
/// header: enumeration is a tool's privilege, not a runtime's.
pub fn discover(assets: &AssetServer, root: &Path) -> Kits {
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Kits(
        found
            .into_iter()
            .map(|(name, relative)| KitEntry {
                handle: assets.load::<Kit>(AssetPath::from(relative)),
                name,
            })
            .collect(),
    )
}

fn walk(root: &Path, dir: &Path, found: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, found);
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".kit.ron") else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        found.push((stem.to_string(), relative.to_path_buf()));
    }
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
