# solari_files

Download San Miguel from <https://casual-effects.com/data> and extract into `raw/` so the OBJ is at
`raw/San_Miguel/san-miguel.obj`, then:

```sh
cargo run --release -p san_miguel_import -- raw/San_Miguel/san-miguel.obj assets/san_miguel san_miguel
```

Download Bistro (Amazon Lumberyard) from <https://developer.nvidia.com/orca/amazon-lumberyard-bistro>
as a PNG-textured `.glb`/`.gltf`, then:

```sh
cargo run --release -p bistro_import -- raw/Bistro/Bistro.glb assets/bistro bistro
```

Baked meshes are cached; pass `--replace` to re-bake. San Miguel also has `--floor-only` and
`--cutout-only` to emit a minimal single-object scene, and `--erode <px>` to tune the OMM cutout
mask. Run an importer with `--help` for all options.


# Terrain paint arrays (Poly Haven)

Downloads material maps from <https://polyhaven.com> (CC0, cached in `raw/polyhaven/`) and bakes
mipped KTX2 `texture_2d_array`s into `assets/terrain/` — albedo (sRGB) + normal + ARM, one layer
per material. With no slugs it bakes the 24-layer set: 0..=10 index-aligned to zero's `BiomeType`
(Ocean..Mountain), 11..=21 per-biome secondary variants (biome id + 11), 22 scree, 23 wet
drainage mud; `terrain_layers.json` records the layer↔slug mapping.

```sh
cargo run --release -p polyhaven                    # 11-biome set at 1k
cargo run --release -p polyhaven -- --res 2k        # same set, 2k
cargo run --release -p polyhaven -- snow_02 ...     # custom layer list
cp assets/terrain/terrain_*.ktx2 /mnt/code/p/zero/assets/terrain/
```

# Trees (SpeedTree, NVIDIA ORCA)

Source: <https://developer.nvidia.com/orca/speedtree>. Drop the pack under `raw/SpeedTree_v2/`
(one dir per tree, each with `HighPoly/*.fbx` + `Textures/*.dds`). Two stages — the `gltf` crate
can't read FBX and `image` can't read DDS, so Blender does both, then the Rust stage bakes clusters
+ an OMM on every leaf cutout.

Stage A — FBX → glb (embedded PNG, base-color alpha preserved):

```sh
mkdir -p raw/SpeedTree_v2/_glb
for d in raw/SpeedTree_v2/*/HighPoly; do
  tree="$(basename "$(dirname "$d")")"
  blender -b --python scripts/speedtree_to_glb.py -- \
    "$d"/*.fbx "raw/SpeedTree_v2/_glb/${tree// /_}.glb"
done
```

Stage B — glb → one `<Tree>.bsn` each + shared `meshes/`/`textures/`:

```sh
cargo run --release -p speedtree_import -- raw/SpeedTree_v2/_glb assets/speedtree
```

Foliage is classified from the base-color alpha histogram (not the glTF alpha mode), so leaf/frond
materials get `AlphaMode::Mask` + a 2-state OMM (100% known → zero any-hit on leaves) while bark/caps
stay opaque. `--replace` re-bakes meshes, `--scale`/`--erode`/`--level` tune size/OMM. FBX author
units are cm; the baked entities carry a 0.01 scale so trees land metric (White Oak ≈ 7.6 m).


# Zero-Day (Beeple, NVIDIA ORCA) — the first ANIMATED asset

Download from <https://developer.nvidia.com/orca/beeple-zero-day> and extract under `raw/ZeroDay_v1/`
(`MEASURE_ONE/`, `MEASURE_SEVEN/`, each a `.fbx` + `tex/*.dds`). Two stages: Blender decodes the FBX +
DXT `.dds` (the Rust crates can't), then `zeroday_import` bakes a **hierarchy-preserving** `.bsn`
(this asset carries rigid TRS animation) plus a compact `.animclip`.

```sh
# Stage A — FBX → geometry glb + meshless anim glb (one run, matching node names)
blender -b --python scripts/zeroday_to_glb.py -- \
  raw/ZeroDay_v1/MEASURE_ONE/MEASURE_ONE.fbx raw/ZeroDay_v1/_glb/measure_one

# Stage B — glb → assets/zeroday/{MeasureOne.bsn, MeasureOne.animclip, meshes/, textures/}
cargo run --release -p zeroday_import -- \
  raw/ZeroDay_v1/_glb/measure_one.glb assets/zeroday MeasureOne
```

Optional checks: `python3 scripts/glb_json.py <file.glb>` (inspect a glb),
`cargo run --release -p zeroday_import --bin load_test -- "$PWD/assets" zeroday/MeasureOne.bsn`
(headless `.bsn` load, no GPU).

Render in zero (solari_files is just the asset factory — copy the output over, then run the example):

```sh
cp -r assets/zeroday /mnt/code/p/zero/assets/
cd /mnt/code/p/zero && cargo run --example zero_day
```

Deferred: DirectX normal green-flip, opacity/transmission, emissive nit tuning.


# Viewer

Run from the repo root, passing a tab-completed path (add `--features dlss` for DLSS):

```sh
cargo run --release -p solari_view -- assets/lunarbase/KB3D_LNB_Bench_A.bsn
```

Pass a glob to load several scenes at once — each match becomes a row in the bottom-left picker;
click a row or press `[`/`]` to switch the live scene:

```sh
cargo run --release -p solari_view -- 'assets/lunarbase/KB3D_LNB_BldgLG*'
```

Quote the glob so the viewer expands it (an unquoted glob the shell already expanded works too). With
many matches the picker can run taller than the window; `[`/`]` still cycle through every scene even
when a row is clipped off-screen.

Or install the `solari_view` binary and run it from here:

```sh
cargo install --path crates/viewer
solari_view assets/lunarbase/KB3D_LNB_Bench_A.bsn
```



# Vulkan 

To install lastest sdk
```bash
curl -L https://sdk.lunarg.com/sdk/download/1.4.350.1/linux/vulkansdk-linux-x86_64-1.4.350.1.tar.xz \
  -o /tmp/vulkansdk-1.4.350.1.tar.xz
sudo mkdir -p /opt/vulkan/1.4.350.1
sudo tar -xf /tmp/vulkansdk-1.4.350.1.tar.xz -C /opt/vulkan/1.4.350.1 --strip-components=1
```

set in shell

```bash
source /opt/vulkan/1.4.350.1/setup-env.sh
```

test its working

```bash
echo "$VULKAN_SDK"   # should now read .../1.4.350.1/x86_64
```