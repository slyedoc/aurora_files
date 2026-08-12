use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

// Common required files for parts with mhclo
const COMMON_ITEMS: [&str; 4] = ["mhclo", "mhmat", "obj", "thumb"];

// Parts that derive Component directly (used as components without wrappers)
const COMPONENT_ENUMS: &[&str] = &[
    "Hair",
    "Eyebrows",
    "Eyelashes",
    "Teeth",
    "Tongue",
    "Eyes",
    "SkinMesh",
    "SkinMaterial",
];

fn main() -> io::Result<()> {
    // for (key, value) in env::vars() {
    //     println!("cargo:warning=Env {}={}", key, value);
    // }

    // The curated MakeHuman library lives at the aurora_files workspace root
    // (raw/ is machine-local, gitignored); apps point their AssetServer root
    // at raw/ so the generated "make_human/…" paths resolve.
    let assets_dir = get_base_path().join("../../raw").join("make_human");

    println!("cargo:rerun-if-changed=build.rs");
    // NOT `{:?}`: Debug-quoting the path used to break rerun-if-changed
    // entirely (cargo saw a quoted literal that never matched).
    println!("cargo:rerun-if-changed={}", assets_dir.display());

    if !assets_dir.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("assets dir not found: {:?}", assets_dir),
        ));
    }

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("assets.rs");
    let mut f = File::create(dest_path)?;

    // Proxymeshes -> SkinMesh
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "proxymeshes",
        "SkinMesh",
        &AssetFilePattern {
            required: &["obj", "proxy", "thumb"],
            textures: &[],
        },
    )?;

    // Rigs
    generate_rig_enum(&mut f, &assets_dir)?;

    // Skins -> SkinMaterial
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "skins",
        "SkinMaterial",
        &AssetFilePattern {
            required: &["mhmat", "thumb"],
            textures: &["diffuse", "normal", "specular"],
        },
    )?;

    // Generate enums for each asset type with specific file patterns
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "hair",
        "Hair",
        &AssetFilePattern {
            required: &["mhclo", "mhmat", "obj", "thumb"],
            textures: &["diffuse", "normal", "specular", "ao", "bump"],
        },
    )?;

    // Clothing
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "clothes",
        "Clothing",
        &AssetFilePattern {
            required: &["mhclo", "mhmat", "obj", "thumb"],
            textures: &["diffuse", "normal", "specular", "ao", "bump"],
        },
    )?;

    // TODO: testing different way, its the only specify asset here that
    // supports different meshes
    // Eyes - cross-product of mesh × material
    generate_eyes_enum(&mut f, &assets_dir)?;

    // Eyebrows - Component enum
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "eyebrows",
        "Eyebrows",
        &AssetFilePattern {
            required: &COMMON_ITEMS,
            textures: &[],
        },
    )?;

    // Eyelashes - Component enum
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "eyelashes",
        "Eyelashes",
        &AssetFilePattern {
            required: &COMMON_ITEMS,
            textures: &[],
        },
    )?;

    // Teeth - Component enum
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "teeth",
        "Teeth",
        &AssetFilePattern {
            required: &COMMON_ITEMS,
            textures: &[],
        },
    )?;

    // Tongue - Component enum
    generate_asset_enum(
        &mut f,
        &assets_dir,
        "tongue",
        "Tongue",
        &AssetFilePattern {
            required: &COMMON_ITEMS,
            textures: &[],
        },
    )?;

    // Poses - BVH files for static poses
    generate_pose_enum(&mut f, &assets_dir)?;

    // Morph targets - read from target.json
    generate_morph_enums(&mut f, &assets_dir)?;

    // MacroMorphs - scanned from macrodetails, breast, height, proportions folders
    generate_macro_morphs(&mut f, &assets_dir)?;

    // MHPart trait for generic part handling
    generate_mhpart_trait(&mut f)?;

    // TODO: not used yet
    // Expression targets - for facial animation (ARKit compatible)
    generate_expression_enum(&mut f, &assets_dir)?;

    Ok(())
}

// from bevy_asset
pub(crate) fn get_base_path() -> PathBuf {
    if let Ok(manifest_dir) = env::var("BEVY_ASSET_ROOT") {
        PathBuf::from(manifest_dir)
    } else if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        PathBuf::from(manifest_dir)
    } else {
        env::current_exe()
            .map(|path| path.parent().map(ToOwned::to_owned).unwrap())
            .unwrap()
    }
}

struct AssetFilePattern {
    required: &'static [&'static str],
    textures: &'static [&'static str],
}

fn generate_asset_enum(
    f: &mut File,
    assets_dir: &Path,
    subdir: &str,
    enum_name: &str,
    pattern: &AssetFilePattern,
) -> io::Result<()> {
    let dir_path = assets_dir.join(subdir);

    if !dir_path.exists() {
        println!("cargo:warning=Skipping {}, directory not found", subdir);
        return Ok(());
    }

    // Collect all subdirectories (each contains one asset)
    let mut entries: Vec<_> = fs::read_dir(&dir_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    entries.sort_by(|a, b| a.path().file_name().cmp(&b.path().file_name()));

    // Write enum with strum derives including EnumProperty
    // Add Component derive for types used directly as components
    let is_component = COMPONENT_ENUMS.contains(&enum_name);
    let component_derive = if is_component { "Component, " } else { "" };
    writeln!(f, "/// Generated from assets/make_human/{}", subdir)?;
    writeln!(
        f,
        "#[derive({}Default, Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Display, EnumProperty, Reflect, Serialize, Deserialize)]",
        component_derive
    )?;
    writeln!(f, "pub enum {} {{", enum_name)?;

    let mut first = true;
    for entry in &entries {
        let dir_name = entry.file_name();
        let dir_name_str = dir_name.to_string_lossy();
        let asset_dir = entry.path();

        // Check if this directory has at least one required file
        let has_required = pattern.required.iter().any(|file_type| {
            asset_dir
                .join(format!("{}.{}", dir_name_str, file_type))
                .exists()
        });

        if !has_required {
            continue;
        }

        let variant_name = sanitize_name(&dir_name_str);

        // Scan for all files in this asset directory
        let mut props = Vec::new();
        // Required files - find and assert they exist
        for file_type in pattern.required {
            // Try exact match first
            let mut file_path = asset_dir.join(format!("{}.{}", dir_name_str, file_type));

            // Fallback: search for ANY file with this extension in the directory
            if !file_path.exists() {
                let found = fs::read_dir(&asset_dir).ok().and_then(|entries| {
                    entries.filter_map(|e| e.ok()).find(|e| {
                        e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| ext == *file_type)
                            .unwrap_or(false)
                    })
                });

                if let Some(found_entry) = found {
                    file_path = found_entry.path();
                } else {
                    panic!(
                        "Required file missing for {}: {} (expected at {:?}, or any *.{} in dir)",
                        enum_name, file_type, asset_dir, file_type
                    );
                }
            }

            let filename = file_path.file_name().unwrap().to_string_lossy();
            let path = format!("make_human/{}/{}/{}", subdir, dir_name_str, filename);
            let clean_name = file_type.replace(".", "_");
            props.push(format!("{} = \"{}\"", clean_name, path));
        }

        // Textures - scan for common texture files
        for texture_type in pattern.textures {
            let file_path = asset_dir.join(format!("{}_{}.png", dir_name_str, texture_type));
            if file_path.exists() {
                let png_path = format!(
                    "make_human/{}/{}/{}_{}.png",
                    subdir, dir_name_str, dir_name_str, texture_type
                );
                props.push(format!("{} = \"{}\"", texture_type, png_path));
            }
        }

        // For proxies, add vertex count to doc
        let doc_suffix = if enum_name == "ProxyMesh" {
            // Count verts from .obj file
            let obj_path = asset_dir.join(format!("{}.obj", dir_name_str));
            if obj_path.exists() {
                if let Ok(content) = fs::read_to_string(&obj_path) {
                    let vert_count = content
                        .lines()
                        .filter(|line| line.starts_with("v "))
                        .count();
                    format!(" ({} verts)", vert_count)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        writeln!(f, "    /// {}{}", dir_name_str, doc_suffix)?;
        if first {
            writeln!(f, "    #[default]")?;
            first = false;
        }
        writeln!(f, "    #[strum(props({}))]", props.join(", "))?;
        writeln!(f, "    {},", variant_name)?;
    }

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Enums that implement MHPart trait - skip generating accessors covered by trait
    const MHPART_ENUMS: &[&str] = &[
        "Eyes",
        "Eyebrows",
        "Eyelashes",
        "Teeth",
        "Tongue",
        "Hair",
        "Clothing",
    ];
    const MHPART_METHODS: &[&str] = &["mhclo", "mhmat", "obj", "thumb"];
    let is_mhpart = MHPART_ENUMS.contains(&enum_name);

    // Generate helper methods using EnumProperty
    // Skip methods covered by MHPart trait
    let has_non_trait_methods = pattern
        .required
        .iter()
        .any(|f| !is_mhpart || !MHPART_METHODS.contains(f))
        || !pattern.textures.is_empty();

    if has_non_trait_methods {
        writeln!(f, "impl {} {{", enum_name)?;

        // Required file accessors (skip if covered by MHPart)
        for file_type in pattern.required {
            let clean_name = file_type.replace(".", "_");
            if is_mhpart && MHPART_METHODS.contains(&clean_name.as_str()) {
                continue;
            }
            writeln!(f, "    /// Get .{} path", file_type)?;
            writeln!(f, "    pub fn {}(&self) -> &'static str {{", clean_name)?;
            writeln!(f, "        self.get_str(\"{}\").unwrap()", clean_name)?;
            writeln!(f, "    }}")?;
            writeln!(f)?;
        }

        // Texture accessors
        for texture_type in pattern.textures {
            writeln!(f, "    /// Get {} texture path if available", texture_type)?;
            writeln!(
                f,
                "    pub fn {}_texture(&self) -> Option<&'static str> {{",
                texture_type
            )?;
            writeln!(f, "        use strum::EnumProperty;")?;
            writeln!(f, "        self.get_str(\"{}\")", texture_type)?;
            writeln!(f, "    }}")?;
            writeln!(f)?;
        }

        writeln!(f, "}}")?;
        writeln!(f)?;
    }

    Ok(())
}

// generate_flat_file_enum removed - Eyes now uses generate_eyes_enum for cross-product

/// Generate Eyes enum as cross-product of mesh variants × material variants
fn generate_eyes_enum(f: &mut File, assets_dir: &Path) -> io::Result<()> {
    let eyes_dir = assets_dir.join("eyes");
    let materials_dir = eyes_dir.join("materials");

    if !eyes_dir.exists() || !materials_dir.exists() {
        println!("cargo:warning=Skipping eyes, directory not found");
        return Ok(());
    }

    // Collect mesh variants (subdirs with mhclo)
    let mut meshes: Vec<(String, String)> = Vec::new(); // (variant_name, dir_name)
    for entry in fs::read_dir(&eyes_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name == "materials" {
            continue;
        }
        // Check for mhclo file
        let mhclo_path = path.join(format!("{}.mhclo", dir_name));
        if mhclo_path.exists() {
            meshes.push((sanitize_name(&dir_name), dir_name));
        }
    }
    meshes.sort_by(|a, b| a.0.cmp(&b.0));

    // Collect material variants
    let mut materials: Vec<(String, String)> = Vec::new(); // (variant_name, file_stem)
    for entry in fs::read_dir(&materials_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mhmat") {
            continue;
        }
        let file_stem = path.file_stem().unwrap().to_string_lossy().to_string();
        materials.push((sanitize_name(&file_stem), file_stem));
    }
    materials.sort_by(|a, b| a.0.cmp(&b.0));

    // Generate cross-product enum
    writeln!(
        f,
        "/// Generated from assets/make_human/eyes (mesh × material cross-product)"
    )?;
    writeln!(
        f,
        "#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Display, EnumProperty, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum Eyes {{")?;

    let mut first = true;
    for (mesh_variant, mesh_dir) in &meshes {
        for (mat_variant, mat_stem) in &materials {
            let variant_name = format!("{}{}", mesh_variant, mat_variant);
            let mhclo_path = format!("make_human/eyes/{}/{}.mhclo", mesh_dir, mesh_dir);
            let obj_path = format!("make_human/eyes/{}/{}.obj", mesh_dir, mesh_dir);
            let mhmat_path = format!("make_human/eyes/materials/{}.mhmat", mat_stem);
            let thumb_path = format!("make_human/eyes/{}/{}.thumb", mesh_dir, mesh_dir);

            writeln!(f, "    /// {} mesh with {} material", mesh_dir, mat_stem)?;
            if first {
                writeln!(f, "    #[default]")?;
                first = false;
            }
            writeln!(
                f,
                "    #[strum(props(mhclo = \"{}\", obj = \"{}\", mhmat = \"{}\", thumb = \"{}\"))]",
                mhclo_path, obj_path, mhmat_path, thumb_path
            )?;
            writeln!(f, "    {},", variant_name)?;
        }
    }

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Eyes implements MHPart trait, no need for duplicate methods

    Ok(())
}

/// Generate PoseAsset enum from poses directory (BVH files)
fn generate_pose_enum(f: &mut File, assets_dir: &Path) -> io::Result<()> {
    let dir_path = assets_dir.join("poses");

    if !dir_path.exists() {
        println!("cargo:warning=Skipping poses, directory not found");
        return Ok(());
    }

    // Collect all subdirectories containing .bvh files
    let mut entries: Vec<_> = fs::read_dir(&dir_path)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            if !path.is_dir() {
                return false;
            }
            // Check for .bvh file in this directory
            let dir_name = e.file_name();
            let bvh_path = path.join(format!("{}.bvh", dir_name.to_string_lossy()));
            bvh_path.exists()
        })
        .collect();

    entries.sort_by(|a, b| a.path().file_name().cmp(&b.path().file_name()));

    writeln!(f, "/// Generated from assets/make_human/poses")?;
    writeln!(
        f,
        "#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Display, EnumProperty, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum PoseAsset {{")?;

    let mut first = true;
    for entry in &entries {
        let dir_name = entry.file_name();
        let dir_name_str = dir_name.to_string_lossy();
        let variant_name = sanitize_name(&dir_name_str);
        let bvh_path = format!("make_human/poses/{}/{}.bvh", dir_name_str, dir_name_str);
        let thumb_path = format!("make_human/poses/{}/{}.thumb", dir_name_str, dir_name_str);

        writeln!(f, "    /// {}", dir_name_str)?;
        if first {
            writeln!(f, "    #[default]")?;
            first = false;
        }
        writeln!(
            f,
            "    #[strum(props(bvh = \"{}\", thumb = \"{}\"))]",
            bvh_path, thumb_path
        )?;
        writeln!(f, "    {},", variant_name)?;
    }

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Generate helper methods
    writeln!(f, "impl PoseAsset {{")?;
    writeln!(f, "    /// Get .bvh path")?;
    writeln!(f, "    pub fn bvh_path(&self) -> &'static str {{")?;
    writeln!(f, "        self.get_str(\"bvh\").unwrap()")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;
    writeln!(f, "    pub fn thumb(&self) -> &'static str {{")?;
    writeln!(f, "        self.get_str(\"thumb\").unwrap()")?;
    writeln!(f, "    }}")?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    Ok(())
}

/// Generate Rig enum - requires rig.json and mhw
fn generate_rig_enum(f: &mut File, assets_dir: &Path) -> io::Result<()> {
    let dir_path = assets_dir.join("rigs");

    if !dir_path.exists() {
        println!("cargo:warning=Skipping rigs, directory not found");
        return Ok(());
    }

    let mut entries: Vec<_> = fs::read_dir(&dir_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    entries.sort_by(|a, b| a.path().file_name().cmp(&b.path().file_name()));

    writeln!(
        f,
        "#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Display, EnumProperty, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum Rig {{")?;

    let mut first = true;
    for entry in &entries {
        let dir_name = entry.file_name();
        let dir_name_str = dir_name.to_string_lossy();
        let asset_dir = entry.path();

        // Check required files exist
        let rig_path = asset_dir.join(format!("{}.rig.json", dir_name_str));
        let weights_path = asset_dir.join(format!("{}.mhw", dir_name_str));

        if !rig_path.exists() || !weights_path.exists() {
            continue;
        }

        let variant_name = sanitize_name(&dir_name_str);

        // Mark first variant as default
        if first {
            writeln!(f, "    #[default]")?;
            first = false;
        }
        let mut props = Vec::new();

        props.push(format!(
            "rig_json = \"make_human/rigs/{}/{}.rig.json\"",
            dir_name_str, dir_name_str
        ));
        props.push(format!(
            "weights = \"make_human/rigs/{}/{}.mhw\"",
            dir_name_str, dir_name_str
        ));

        // Check for optional rotations file
        let rotations_path = asset_dir.join(format!("{}_rotations.json", dir_name_str));
        if rotations_path.exists() {
            props.push(format!(
                "rotations = \"make_human/rigs/{}/{}_rotations.json\"",
                dir_name_str, dir_name_str
            ));
        }

        writeln!(f, "    /// {}", dir_name_str)?;
        writeln!(f, "    #[strum(props({}))]", props.join(", "))?;
        writeln!(f, "    {},", variant_name)?;
    }

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Generate helper methods
    writeln!(f, "impl Rig {{")?;

    writeln!(f, "    pub fn rig_json_path(&self) -> &'static str {{")?;
    writeln!(f, "        self.get_str(\"rig_json\").unwrap()")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(f, "    pub fn weights(&self) -> &'static str {{")?;
    writeln!(f, "        self.get_str(\"weights\").unwrap()")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(f, "    pub fn rotations(&self) -> Option<&'static str> {{")?;
    writeln!(f, "        self.get_str(\"rotations\")")?;
    writeln!(f, "    }}")?;

    writeln!(f)?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    Ok(())
}

fn sanitize_name(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .unwrap()
                .to_uppercase()
                .chain(chars.flat_map(|c| c.to_lowercase()))
                .collect::<String>()
        })
        .collect()
}

/// Pair type determines which suffix means negative vs positive
#[derive(Debug, Clone, Copy, PartialEq)]
enum PairType {
    IncrDecr,           // neg=decr, pos=incr
    UpDown,             // neg=down, pos=up
    InOut,              // neg=in, pos=out
    ForwardBackward,    // neg=backward, pos=forward
    ConvexConcave,      // neg=concave, pos=convex
    CompressUncompress, // neg=compress, pos=uncompress
    SquareRound,        // neg=square, pos=round
    PointedTriangle,    // neg=pointed, pos=triangle
    Single,             // only one target (0..1 range)
}

impl PairType {
    fn from_label(label: &str) -> Self {
        if label.ends_with("-decr-incr") {
            Self::IncrDecr
        } else if label.ends_with("-down-up") {
            Self::UpDown
        } else if label.ends_with("-in-out") {
            Self::InOut
        } else if label.ends_with("-backward-forward") {
            Self::ForwardBackward
        } else if label.ends_with("-concave-convex") || label.ends_with("-convex-concave") {
            Self::ConvexConcave
        } else if label.ends_with("-compress-uncompress") {
            Self::CompressUncompress
        } else if label.ends_with("-square-round") {
            Self::SquareRound
        } else if label.ends_with("-pointed-triangle") {
            Self::PointedTriangle
        } else {
            Self::Single
        }
    }
}

/// Represents a morph (binary pair or single)
#[derive(Debug, Clone)]
struct MorphEntry {
    /// Base name (e.g., "upperarm-fat-l" for sided, "head-age" for unsided)
    name: String,
    /// Pair type determines value range and path selection
    pair_type: PairType,
    /// Path to negative target (for binary pairs)
    neg_path: Option<String>,
    /// Path to positive target (for binary pairs) or only path (for singles)
    pos_path: Option<String>,
}

fn generate_morph_enums(f: &mut File, assets_dir: &Path) -> io::Result<()> {
    // Read target.json to get categories
    let target_json_path = assets_dir.join("targets/target.json");
    let json_str = fs::read_to_string(&target_json_path)?;
    let json: serde_json::Value =
        serde_json::from_str(&json_str).expect("Failed to parse target.json");

    let categories = json.as_object().expect("target.json must be object");

    let mut category_names: Vec<String> = categories
        .keys()
        .filter(|k| *k != "measure") // skip measure category
        .cloned()
        .collect();
    category_names.sort();

    // Collect morph entries per category
    let mut entries_by_category: HashMap<String, Vec<MorphEntry>> = HashMap::new();

    for (category, data) in categories {
        if category == "measure" {
            continue;
        }

        let mut entries: Vec<MorphEntry> = Vec::new();
        let mut seen_bases: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Some(cats) = data.get("categories").and_then(|v| v.as_array()) {
            for cat in cats {
                let label = cat.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let pair_type = PairType::from_label(label);
                let opposites = cat.get("opposites");

                if pair_type == PairType::Single {
                    // Single target (no opposites or empty opposites)
                    if let Some(targets) = cat.get("targets").and_then(|v| v.as_array()) {
                        for target in targets {
                            if let Some(target_name) = target.as_str() {
                                if !seen_bases.contains(target_name) {
                                    seen_bases.insert(target_name.to_string());
                                    entries.push(MorphEntry {
                                        name: target_name.to_string(),
                                        pair_type: PairType::Single,
                                        neg_path: None,
                                        pos_path: Some(format!(
                                            "make_human/targets/{}/{}.target",
                                            category, target_name
                                        )),
                                    });
                                }
                            }
                        }
                    }
                } else if let Some(opp) = opposites.and_then(|v| v.as_object()) {
                    // Binary pair with opposites
                    let pos_left = opp
                        .get("positive-left")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let neg_left = opp
                        .get("negative-left")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let pos_right = opp
                        .get("positive-right")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let neg_right = opp
                        .get("negative-right")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let pos_unsided = opp
                        .get("positive-unsided")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let neg_unsided = opp
                        .get("negative-unsided")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());

                    // Left side - use suffix L
                    if pos_left.is_some() || neg_left.is_some() {
                        let base = extract_base_name(pos_left.or(neg_left).unwrap(), pair_type);
                        let sided_name = format!("{}-l", base);
                        if !seen_bases.contains(&sided_name) {
                            seen_bases.insert(sided_name.clone());
                            entries.push(MorphEntry {
                                name: sided_name,
                                pair_type,
                                neg_path: neg_left.map(|t| {
                                    format!("make_human/targets/{}/{}.target", category, t)
                                }),
                                pos_path: pos_left.map(|t| {
                                    format!("make_human/targets/{}/{}.target", category, t)
                                }),
                            });
                        }
                    }

                    // Right side - use suffix R
                    if pos_right.is_some() || neg_right.is_some() {
                        let base = extract_base_name(pos_right.or(neg_right).unwrap(), pair_type);
                        let sided_name = format!("{}-r", base);
                        if !seen_bases.contains(&sided_name) {
                            seen_bases.insert(sided_name.clone());
                            entries.push(MorphEntry {
                                name: sided_name,
                                pair_type,
                                neg_path: neg_right.map(|t| {
                                    format!("make_human/targets/{}/{}.target", category, t)
                                }),
                                pos_path: pos_right.map(|t| {
                                    format!("make_human/targets/{}/{}.target", category, t)
                                }),
                            });
                        }
                    }

                    // Unsided
                    if pos_unsided.is_some() || neg_unsided.is_some() {
                        let base =
                            extract_base_name(pos_unsided.or(neg_unsided).unwrap(), pair_type);
                        if !seen_bases.contains(&base) {
                            seen_bases.insert(base.clone());
                            entries.push(MorphEntry {
                                name: base,
                                pair_type,
                                neg_path: neg_unsided.map(|t| {
                                    format!("make_human/targets/{}/{}.target", category, t)
                                }),
                                pos_path: pos_unsided.map(|t| {
                                    format!("make_human/targets/{}/{}.target", category, t)
                                }),
                            });
                        }
                    }
                }
            }
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries_by_category.insert(category.clone(), entries);
    }

    // Generate enum per category
    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();

        if entries.is_empty() {
            continue;
        }

        let enum_name = format!("{}Morph", sanitize_name(category));

        writeln!(f, "/// Generated from target.json category: {}", category)?;
        writeln!(f, "/// Binary pairs: -1.0 to 1.0 (neg/pos targets)")?;
        writeln!(f, "/// Singles: 0.0 to 1.0 (one target)")?;
        writeln!(
            f,
            "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Display, EnumProperty, Reflect, Serialize, Deserialize)]"
        )?;
        writeln!(f, "pub enum {} {{", enum_name)?;

        for entry in entries {
            let variant_name = sanitize_name(&entry.name);
            let mut props = Vec::new();

            if let Some(ref path) = entry.pos_path {
                props.push(format!("pos = \"{}\"", path));
            }
            if let Some(ref path) = entry.neg_path {
                props.push(format!("neg = \"{}\"", path));
            }
            if entry.pair_type == PairType::Single {
                props.push("single = \"true\"".to_string());
            }

            let range = if entry.pair_type == PairType::Single {
                "0.0 to 1.0"
            } else {
                "-1.0 to 1.0"
            };
            writeln!(f, "    /// {} ({})", entry.name, range)?;
            if !props.is_empty() {
                writeln!(f, "    #[strum(props({}))]", props.join(", "))?;
            }
            writeln!(f, "    {},", variant_name)?;
        }

        writeln!(f, "}}")?;
        writeln!(f)?;

        // Impl
        writeln!(f, "impl {} {{", enum_name)?;
        writeln!(f, "    /// Get positive target path")?;
        writeln!(f, "    pub fn pos_path(&self) -> Option<&'static str> {{")?;
        writeln!(f, "        self.get_str(\"pos\")")?;
        writeln!(f, "    }}")?;
        writeln!(f)?;
        writeln!(f, "    /// Get negative target path")?;
        writeln!(f, "    pub fn neg_path(&self) -> Option<&'static str> {{")?;
        writeln!(f, "        self.get_str(\"neg\")")?;
        writeln!(f, "    }}")?;
        writeln!(f)?;
        writeln!(
            f,
            "    /// Check if this is a single (0..1) vs binary (-1..1)"
        )?;
        writeln!(f, "    pub fn is_single(&self) -> bool {{")?;
        writeln!(f, "        self.get_str(\"single\").is_some()")?;
        writeln!(f, "    }}")?;
        writeln!(f)?;
        writeln!(f, "    /// Get target path based on sign of value")?;
        writeln!(f, "    /// For singles, always returns pos_path")?;
        writeln!(
            f,
            "    /// For binary, neg value -> neg_path, pos value -> pos_path"
        )?;
        writeln!(
            f,
            "    pub fn target_path(&self, value: f32) -> Option<&'static str> {{"
        )?;
        writeln!(
            f,
            "        if self.is_single() || value >= 0.0 {{ self.pos_path() }} else {{ self.neg_path() }}"
        )?;
        writeln!(f, "    }}")?;
        writeln!(f)?;
        writeln!(f, "    /// Get valid value range: (min, max)")?;
        writeln!(f, "    pub fn value_range(&self) -> (f32, f32) {{")?;
        writeln!(
            f,
            "        if self.is_single() {{ (0.0, 1.0) }} else {{ (-1.0, 1.0) }}"
        )?;
        writeln!(f, "    }}")?;
        writeln!(f)?;
        writeln!(f, "}}")?;
        writeln!(f)?;
    }

    // Generate unified MorphTarget enum
    writeln!(f, "/// Unified morph target enum containing all categories")?;
    writeln!(f, "/// Binary pairs: -1.0 to 1.0, Singles: 0.0 to 1.0")?;
    writeln!(
        f,
        "/// Macro morphs: 0.0 to 1.0 (macrodetails, height, proportions, breast)"
    )?;
    writeln!(
        f,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum MorphTarget {{")?;

    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let enum_name = format!("{}Morph", sanitize_name(category));
        let variant = sanitize_name(category);
        writeln!(f, "    {}({}),", variant, enum_name)?;
    }
    // Add Macro variant for macro morphs
    writeln!(
        f,
        "    /// Macro morphs (macrodetails, height, proportions, breast)"
    )?;
    writeln!(f, "    Macro(MacroMorph),")?;

    writeln!(f, "}}")?;
    writeln!(f)?;

    writeln!(f, "impl MorphTarget {{")?;
    writeln!(
        f,
        "    /// Get target path based on sign of value (for simple morphs)"
    )?;
    writeln!(
        f,
        "    /// For simple macro morphs returns avg path, for interpolated returns None"
    )?;
    writeln!(
        f,
        "    pub fn target_path(&self, value: f32) -> Option<&'static str> {{"
    )?;
    writeln!(f, "        match self {{")?;

    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let variant = sanitize_name(category);
        writeln!(
            f,
            "            Self::{}(m) => m.target_path(value),",
            variant
        )?;
    }
    writeln!(f, "            Self::Macro(m) => {{")?;
    writeln!(
        f,
        "                if m.is_interpolated() {{ None }} else {{ m.paths().1 }}"
    )?;
    writeln!(f, "            }}")?;

    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(
        f,
        "    /// Get interpolation paths for macro morphs (min, avg, max)"
    )?;
    writeln!(f, "    /// Returns None for body morphs")?;
    writeln!(
        f,
        "    pub fn macro_paths(&self) -> Option<(Option<&'static str>, Option<&'static str>, Option<&'static str>)> {{"
    )?;
    writeln!(f, "        match self {{")?;
    writeln!(f, "            Self::Macro(m) => Some(m.paths()),")?;
    writeln!(f, "            _ => None,")?;
    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(f, "    /// Check if this is an interpolated macro morph")?;
    writeln!(f, "    pub fn is_interpolated(&self) -> bool {{")?;
    writeln!(f, "        match self {{")?;
    writeln!(f, "            Self::Macro(m) => m.is_interpolated(),")?;
    writeln!(f, "            _ => false,")?;
    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(f, "    /// Get positive target path")?;
    writeln!(f, "    pub fn pos_path(&self) -> Option<&'static str> {{")?;
    writeln!(f, "        match self {{")?;

    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let variant = sanitize_name(category);
        writeln!(f, "            Self::{}(m) => m.pos_path(),", variant)?;
    }
    writeln!(f, "            Self::Macro(m) => m.paths().1,")?;

    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(
        f,
        "    /// Get negative target path (None for macro morphs)"
    )?;
    writeln!(f, "    pub fn neg_path(&self) -> Option<&'static str> {{")?;
    writeln!(f, "        match self {{")?;

    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let variant = sanitize_name(category);
        writeln!(f, "            Self::{}(m) => m.neg_path(),", variant)?;
    }
    writeln!(f, "            Self::Macro(_) => None,")?;

    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(
        f,
        "    /// Check if this is a single (0..1) vs binary (-1..1)"
    )?;
    writeln!(f, "    /// Macro morphs are always single")?;
    writeln!(f, "    pub fn is_single(&self) -> bool {{")?;
    writeln!(f, "        match self {{")?;

    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let variant = sanitize_name(category);
        writeln!(f, "            Self::{}(m) => m.is_single(),", variant)?;
    }
    writeln!(f, "            Self::Macro(_) => true,")?;

    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(f, "    /// Get valid value range: (min, max)")?;
    writeln!(f, "    pub fn value_range(&self) -> (f32, f32) {{")?;
    writeln!(f, "        match self {{")?;

    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let variant = sanitize_name(category);
        writeln!(f, "            Self::{}(m) => m.value_range(),", variant)?;
    }
    writeln!(f, "            Self::Macro(_) => (0.0, 1.0),")?;

    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    // Add iter() method that collects all variants from sub-enums
    writeln!(
        f,
        "    /// Iterate over all morph targets across all categories"
    )?;
    writeln!(f, "    pub fn iter() -> impl Iterator<Item = Self> {{")?;
    writeln!(f, "        use strum::IntoEnumIterator;")?;
    writeln!(f, "        std::iter::empty()")?;
    for category in &category_names {
        let entries = entries_by_category.get(category).unwrap();
        if entries.is_empty() {
            continue;
        }
        let variant = sanitize_name(category);
        let enum_name = format!("{}Morph", variant);
        writeln!(
            f,
            "            .chain({}::iter().map(Self::{}))",
            enum_name, variant
        )?;
    }
    writeln!(f, "            .chain(MacroMorph::iter().map(Self::Macro))")?;
    writeln!(f, "    }}")?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    Ok(())
}

/// Extract base name by removing pair-type suffix and l-/r- prefix
fn extract_base_name(target_name: &str, pair_type: PairType) -> String {
    let mut name = target_name.to_string();

    // Remove l-/r- prefix (will add back as suffix)
    let is_left = name.starts_with("l-");
    let is_right = name.starts_with("r-");
    if is_left || is_right {
        name = name[2..].to_string();
    }

    // Remove pair-type specific suffixes
    let suffixes = match pair_type {
        PairType::IncrDecr => vec!["-incr", "-decr"],
        PairType::UpDown => vec!["-up", "-down"],
        PairType::InOut => vec!["-in", "-out"],
        PairType::ForwardBackward => vec!["-forward", "-backward"],
        PairType::ConvexConcave => vec!["-convex", "-concave"],
        PairType::CompressUncompress => vec!["-compress", "-uncompress"],
        PairType::SquareRound => vec!["-square", "-round"],
        PairType::PointedTriangle => vec!["-pointed", "-triangle"],
        PairType::Single => vec![],
    };

    for suffix in suffixes {
        if name.ends_with(suffix) {
            name = name[..name.len() - suffix.len()].to_string();
            break;
        }
    }

    name
}

/// Expression target names from assets/make_human/targets/expression/units/
/// These are per-ethnicity (caucasian/african/asian)
const EXPRESSION_TARGETS: &[&str] = &[
    "eye-left-closure",
    "eye-left-opened-up",
    "eye-left-slit",
    "eye-right-closure",
    "eye-right-opened-up",
    "eye-right-slit",
    "eyebrows-left-down",
    "eyebrows-left-extern-up",
    "eyebrows-left-inner-up",
    "eyebrows-left-up",
    "eyebrows-right-down",
    "eyebrows-right-extern-up",
    "eyebrows-right-inner-up",
    "eyebrows-right-up",
    "mouth-compression",
    "mouth-corner-puller",
    "mouth-depression",
    "mouth-depression-retraction",
    "mouth-elevation",
    "mouth-eversion",
    "mouth-open",
    "mouth-parling",
    "mouth-part-later",
    "mouth-protusion",
    "mouth-pursing",
    "mouth-retraction",
    "mouth-upward-retraction",
    "neck-platysma",
    "nose-compression",
    "nose-depression",
    "nose-left-dilatation",
    "nose-left-elevation",
    "nose-right-dilatation",
    "nose-right-elevation",
];

/// Generate Expression enum for facial animation targets
fn generate_expression_enum(f: &mut File, assets_dir: &Path) -> io::Result<()> {
    // Verify the targets exist
    let expr_dir = assets_dir.join("targets/expression/units/caucasian");
    if !expr_dir.exists() {
        println!(
            "cargo:warning=Expression targets directory not found: {:?}",
            expr_dir
        );
        return Ok(());
    }

    // Ethnicity enum for expression targets
    writeln!(f, "/// Ethnicity for expression targets")?;
    writeln!(
        f,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, EnumIter, Display, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum Ethnicity {{")?;
    writeln!(f, "    #[default]")?;
    writeln!(f, "    Caucasian,")?;
    writeln!(f, "    African,")?;
    writeln!(f, "    Asian,")?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    writeln!(f, "impl Ethnicity {{")?;
    writeln!(f, "    pub fn as_str(&self) -> &'static str {{")?;
    writeln!(f, "        match self {{")?;
    writeln!(f, "            Self::Caucasian => \"caucasian\",")?;
    writeln!(f, "            Self::African => \"african\",")?;
    writeln!(f, "            Self::Asian => \"asian\",")?;
    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    writeln!(f, "/// Expression targets for facial animation")?;
    writeln!(f, "/// All values are 0.0 to 1.0 (single-sided morphs)")?;
    writeln!(
        f,
        "/// Use with Ethnicity to get ethnicity-specific target path"
    )?;
    writeln!(
        f,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Display, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum Expression {{")?;

    for target in EXPRESSION_TARGETS {
        let variant = sanitize_name(target);
        writeln!(f, "    /// {} (0.0 to 1.0)", target)?;
        writeln!(f, "    {},", variant)?;
    }

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Impl block
    writeln!(f, "impl Expression {{")?;

    // Get target name
    writeln!(
        f,
        "    /// Get the target file name (without path or extension)"
    )?;
    writeln!(f, "    pub fn target_name(&self) -> &'static str {{")?;
    writeln!(f, "        match self {{")?;
    for target in EXPRESSION_TARGETS {
        let variant = sanitize_name(target);
        writeln!(f, "            Self::{} => \"{}\",", variant, target)?;
    }
    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    // Get target path for a specific ethnicity
    writeln!(
        f,
        "    /// Get the full asset path for this expression target"
    )?;
    writeln!(
        f,
        "    /// Uses the specified ethnicity for ethnicity-specific morphs"
    )?;
    writeln!(
        f,
        "    pub fn target_path(&self, ethnicity: Ethnicity) -> String {{"
    )?;
    writeln!(
        f,
        "        format!(\"make_human/targets/expression/units/{{}}/{{}}.target\", ethnicity.as_str(), self.target_name())"
    )?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    // Get target path with default caucasian
    writeln!(
        f,
        "    /// Get the asset path using caucasian variant (default)"
    )?;
    writeln!(f, "    pub fn default_path(&self) -> &'static str {{")?;
    writeln!(f, "        match self {{")?;
    for target in EXPRESSION_TARGETS {
        let variant = sanitize_name(target);
        writeln!(
            f,
            "            Self::{} => \"make_human/targets/expression/units/caucasian/{}.target\",",
            variant, target
        )?;
    }
    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    Ok(())
}

/// Interpolation suffixes - last segment of filename that indicates min/avg/max
const INTERP_SUFFIXES: &[(&str, &str)] = &[
    ("-minweight", "weight"),
    ("-averageweight", "weight"),
    ("-maxweight", "weight"),
    ("-minheight", "height"),
    ("-maxheight", "height"),
    ("-idealproportions", "proportions"),
    ("-uncommonproportions", "proportions"),
];

/// Try to split filename into (base, suffix, group) if it ends with an interp suffix
fn split_interp_suffix(name: &str) -> Option<(&str, &str, &str)> {
    for (suffix, group) in INTERP_SUFFIXES {
        if name.ends_with(suffix) {
            let base = &name[..name.len() - suffix.len()];
            return Some((base, *suffix, *group));
        }
    }
    None
}

/// Macro morph data - either single path or interpolated group
#[derive(Debug)]
struct MacroMorphData {
    variant: String,
    /// For single morphs: (None, path, None). For interp: (min, avg, max) paths
    paths: (Option<String>, Option<String>, Option<String>),
}

/// Generate MacroMorph enum by scanning macro target folders
fn generate_macro_morphs(f: &mut File, assets_dir: &Path) -> io::Result<()> {
    let targets_dir = assets_dir.join("targets");

    let macro_folders = [
        ("macrodetails", ""),
        ("macrodetails/height", "Height"),
        ("macrodetails/proportions", "Proportions"),
        ("breast", "Breast"),
    ];

    // Group files by base name for interpolation
    let mut interp_groups: std::collections::HashMap<
        String,
        (Option<String>, Option<String>, Option<String>),
    > = std::collections::HashMap::new();
    let mut singles: Vec<(String, String)> = Vec::new(); // (variant, path)

    for (folder, prefix) in &macro_folders {
        let folder_path = targets_dir.join(folder);
        if !folder_path.exists() {
            continue;
        }

        let entries: Vec<_> = fs::read_dir(&folder_path)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext == "target")
                    .unwrap_or(false)
            })
            .collect();

        for entry in entries {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            let base_name = name.trim_end_matches(".target");
            let path = format!("make_human/targets/{}/{}.target", folder, base_name);

            if let Some((base, suffix, _group)) = split_interp_suffix(base_name) {
                // Part of interpolation group
                let key = format!("{}:{}", prefix, base);
                let entry = interp_groups.entry(key).or_insert((None, None, None));

                if suffix.contains("min") || suffix.contains("uncommon") {
                    entry.0 = Some(path);
                } else if suffix.contains("average") {
                    entry.1 = Some(path);
                } else if suffix.contains("max") || suffix.contains("ideal") {
                    entry.2 = Some(path);
                }
            } else {
                // Single file
                let variant = if prefix.is_empty() {
                    sanitize_name(base_name)
                } else {
                    format!("{}{}", prefix, sanitize_name(base_name))
                };
                singles.push((variant, path));
            }
        }
    }

    // Build final list
    let mut all_morphs: Vec<MacroMorphData> = Vec::new();

    // Add singles
    for (variant, path) in singles {
        all_morphs.push(MacroMorphData {
            variant,
            paths: (None, Some(path), None),
        });
    }

    // Add interpolation groups
    for (key, paths) in interp_groups {
        let parts: Vec<&str> = key.splitn(2, ':').collect();
        let prefix = parts[0];
        let base = parts.get(1).unwrap_or(&"");

        let variant = if prefix.is_empty() {
            sanitize_name(base)
        } else {
            format!("{}{}", prefix, sanitize_name(base))
        };

        all_morphs.push(MacroMorphData { variant, paths });
    }

    // Sort by variant name
    all_morphs.sort_by(|a, b| a.variant.cmp(&b.variant));

    // Generate enum
    writeln!(
        f,
        "/// Macro morph targets from macrodetails, height, proportions, breast folders"
    )?;
    writeln!(f, "/// Single morphs: value 0-1 scales the morph")?;
    writeln!(f, "/// Interpolated morphs: value 0-1 blends min->avg->max")?;
    writeln!(
        f,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumCount, Reflect, Serialize, Deserialize)]"
    )?;
    writeln!(f, "pub enum MacroMorph {{")?;

    for morph in &all_morphs {
        writeln!(f, "    {},", morph.variant)?;
    }

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Impl block
    writeln!(f, "impl MacroMorph {{")?;

    // paths() method - returns (min, avg, max) Option paths
    writeln!(f, "    /// Get interpolation paths (min, avg, max)")?;
    writeln!(f, "    /// Single morphs return (None, Some(path), None)")?;
    writeln!(
        f,
        "    pub fn paths(&self) -> (Option<&'static str>, Option<&'static str>, Option<&'static str>) {{"
    )?;
    writeln!(f, "        match self {{")?;

    for morph in &all_morphs {
        let min = morph
            .paths
            .0
            .as_ref()
            .map(|p| format!("Some(\"{}\")", p))
            .unwrap_or_else(|| "None".to_string());
        let avg = morph
            .paths
            .1
            .as_ref()
            .map(|p| format!("Some(\"{}\")", p))
            .unwrap_or_else(|| "None".to_string());
        let max = morph
            .paths
            .2
            .as_ref()
            .map(|p| format!("Some(\"{}\")", p))
            .unwrap_or_else(|| "None".to_string());
        writeln!(
            f,
            "            Self::{} => ({}, {}, {}),",
            morph.variant, min, avg, max
        )?;
    }

    writeln!(f, "        }}")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    // is_interpolated() method
    writeln!(
        f,
        "    /// Check if this morph interpolates between multiple targets"
    )?;
    writeln!(f, "    pub fn is_interpolated(&self) -> bool {{")?;
    writeln!(f, "        let (min, _, max) = self.paths();")?;
    writeln!(f, "        min.is_some() || max.is_some()")?;
    writeln!(f, "    }}")?;
    writeln!(f)?;

    writeln!(
        f,
        "    /// Get valid value range (always 0.0 to 1.0 for macro morphs)"
    )?;
    writeln!(f, "    pub fn value_range(&self) -> (f32, f32) {{")?;
    writeln!(f, "        (0.0, 1.0)")?;
    writeln!(f, "    }}")?;

    writeln!(f, "}}")?;
    writeln!(f)?;

    // Display impl
    writeln!(f, "impl std::fmt::Display for MacroMorph {{")?;
    writeln!(
        f,
        "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{"
    )?;
    writeln!(f, "        write!(f, \"{{:?}}\", self)")?;
    writeln!(f, "    }}")?;
    writeln!(f, "}}")?;
    writeln!(f)?;

    Ok(())
}

/// Generate MHThumb and MHPart impl blocks for part enums (traits defined in assets.rs)
fn generate_mhpart_trait(f: &mut File) -> io::Result<()> {
    // Enums with thumb only (no mhclo/mhmat/obj)
    let thumb_only = ["SkinMaterial", "SkinMesh"];

    // Enums with full MHPart (mhclo/mhmat/obj/thumb)
    let full_parts = [
        "Eyes",
        "Eyebrows",
        "Eyelashes",
        "Teeth",
        "Tongue",
        "Hair",
        "Clothing",
    ];

    // Generate MHThumb for thumb-only enums
    for enum_name in thumb_only {
        writeln!(f, "impl MHThumb for {} {{", enum_name)?;
        writeln!(
            f,
            "    fn thumb(&self) -> &'static str {{ self.get_str(\"thumb\").unwrap() }}"
        )?;
        writeln!(f, "}}")?;
        writeln!(f)?;
    }

    // Generate MHThumb + MHPart for full part enums
    for enum_name in full_parts {
        writeln!(f, "impl MHThumb for {} {{", enum_name)?;
        writeln!(
            f,
            "    fn thumb(&self) -> &'static str {{ self.get_str(\"thumb\").unwrap() }}"
        )?;
        writeln!(f, "}}")?;
        writeln!(f)?;

        writeln!(f, "impl MHPart for {} {{", enum_name)?;
        writeln!(
            f,
            "    fn mhclo(&self) -> &'static str {{ self.get_str(\"mhclo\").unwrap() }}"
        )?;
        writeln!(
            f,
            "    fn mhmat(&self) -> &'static str {{ self.get_str(\"mhmat\").unwrap() }}"
        )?;
        writeln!(
            f,
            "    fn obj(&self) -> &'static str {{ self.get_str(\"obj\").unwrap() }}"
        )?;
        writeln!(f, "}}")?;
        writeln!(f)?;
    }

    Ok(())
}
