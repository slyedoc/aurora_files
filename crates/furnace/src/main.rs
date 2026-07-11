//! Furnace validation for the reference path tracer. Three scenes:
//! `furnace` (default): spheres in a uniform white environment — a white sphere of
//! ANY roughness must converge to exactly the sky's radiance (ratio 1.0) or the
//! BRDF creates/destroys energy. `room`: a static grey box of boxes, ramps, and
//! spheres-on-pillars under a lamp grid — the spatial-bias study. `yard`: an open
//! sun + lamp-cluster ground — the directional-vs-reservoir exam.
//! Probe pixels are read back from the RT output buffer and logged as numbers.

use bevy::prelude::*;

use bevy::{
    window::{PresentMode, PrimaryWindow, WindowResolution},
    asset::RenderAssetUsages,
    camera::CameraMainTextureUsages,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    core_pipeline::Skybox,
    dev_tools::render_debug::RenderDebugOverlayPlugin,
    feathers::{dark_theme::create_dark_theme, theme::UiTheme, FeathersPlugins},
    image::{Image, ImageAddressMode, ImageSamplerDescriptor},
    log::LogPlugin,
    math::{DQuat, DVec3},
    pbr::PbrPlugin,
    render::{
        camera::ExtractedCamera,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::{
            Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, MapMode,
            PollType, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
            TextureViewDimension,
        },
        renderer::{RenderDevice, RenderQueue},
        view::{
            screenshot::{save_to_disk, Screenshot, ScreenshotCaptured},
            ExtractedView,
        },
        Render, RenderApp, RenderSystems,
    },
    solari::{
        prelude::*,
        render::rt_pipeline::{RtAccumulation, RtOutputBuffer},
    },
    transform::TransformPlugin,
};
use clap::{Parser, ValueEnum};

// The full-RT path supplies its own DLSS Ray Reconstruction internally; the SDK
// only needs the project id inserted before `RenderPlugin`.
use bevy::anti_alias::dlss::DlssProjectId;

/// Furnace exam harness for the solari reference path tracer (see
/// zero/docs/restir_roadmap.md for the rung-by-rung curriculum this drives).
/// The estimator levers live in the shared [`SolariCameraArgs`] block, so
/// every solari tool speaks the same camera dialect.
/// The exam scenes. `--help` lists these; anything else is a clap error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Scene {
    /// uniform white sky + spheres — any-roughness white sphere must converge to
    /// the sky's radiance (ratio 1.0) or the BRDF is creating/destroying energy
    Furnace,
    /// static grey box: boxes, tilted ramps, and spheres-on-pillars under a lamp
    /// grid — the spatial-bias study (nothing moves, so temporal reuse stays clean)
    Room,
    /// open yard: sun (directional/NEE) + a lamp cluster (reservoir) over one
    /// ground — the directional-vs-reservoir bookkeeping exam
    Yard,
    /// the importer-baked bistro `.bsn` under sun + atmosphere — real content
    /// (glass, foliage cutouts, dense geometry) graded with the same protocols
    Bistro,
}

#[derive(Parser)]
struct Args {
    /// exit after this many seconds (agent runs default to 10)
    #[arg(long, short = 't')]
    timeout: Option<f32>,

    /// which scene to render
    #[arg(long, value_enum, default_value_t = Scene::Furnace)]
    scene: Scene,

    /// force uniform light picking (rung-1 A/B: must converge to the same image
    /// as power-weighted, just noisier)
    #[arg(long)]
    uniform_lights: bool,

    /// write one EXR the moment the accumulation crosses N spp (equal-sample RMSE)
    #[arg(long, default_value_t = 0)]
    dump_at_spp: u32,

    /// write one EXR after N seconds (equal-time comparisons — the grader's
    /// accuracy protocol; pair with `-t` slightly larger to exit after)
    #[arg(long)]
    dump_at_secs: Option<f32>,

    /// screenshot the final window surface after N seconds — post-DLSS-RR,
    /// post-tonemap, the image the user sees (the grader's per-frame capture;
    /// pair with --no-ui or the panels are in the shot)
    #[arg(long)]
    shot_at_secs: Option<f32>,

    /// capture this many CONSECUTIVE frames when --shot-at-secs fires (a
    /// burst): pairwise FLIP between neighbors is the flicker metric —
    /// temporally-sticky error DLSS preserves scores low, per-frame flux high
    #[arg(long, default_value_t = 1)]
    shot_frames: u32,

    /// also screenshot when the --dump-at-spp EXR lands (converged truth gets
    /// a display-referred twin for the per-frame protocol to grade against)
    #[arg(long)]
    shot_on_dump: bool,

    /// run DLSS Ray Reconstruction (DLAA) — the production denoiser
    #[arg(long)]
    dlss: bool,

    /// don't spawn the solari debug panels (screenshot capture runs)
    #[arg(long)]
    no_ui: bool,

    /// terminate GI paths via an inline coopvec MLP query in raygen instead
    /// of the batched query→infer→composite path
    #[arg(long)]
    nrc_inline: bool,

    /// NRC spread-termination threshold c (paper §5.1 footprint gate; larger
    /// = deeper paths / fewer cache queries)
    #[arg(long, default_value_t = 0.01)]
    nrc_spread_c: f32,

    /// log frame-time diagnostics every second (perf comparisons)
    #[arg(long)]
    fps: bool,

    /// sway/yaw the camera for N FRAMES, then snap back to the start pose
    /// and hold. Frame-indexed, not time-based: frame k renders the same pose
    /// at any frame rate, so motion captures compare equal across recipes.
    /// Large N = continuous motion (the grader's --orbit mode)
    #[arg(long, default_value_t = 0)]
    orbit: u32,

    /// room-grid power law: `pilot` (a few floods over a sea of pilots — global
    /// flux weighting wins) or `equal` (identical lamps — the receiver-locality
    /// exam where global picking is uninformative and RIS must find the lamps overhead)
    #[arg(long, default_value = "pilot")]
    lamp_law: String,

    /// the camera estimator levers (shared across every solari tool)
    #[command(flatten)]
    camera: SolariCameraArgs,
}

/// Uniform sky radiance (cd/m²) for the furnace scene.
const SKY_NITS: f32 = 1000.0;

/// Fingerprint of this file's source — scene geometry lives here, so any edit
/// changes the fingerprint and the grader knows its cached truths may show a
/// different scene (dims alone can't tell). Coarse on purpose: a needless
/// rebake after an unrelated edit costs minutes; a stale truth silently
/// poisons every number.
const SCENE_FP: u64 = fnv1a(include_bytes!("main.rs"));

const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

fn main() {
    let mut args = Args::parse();
    // The shipped default IS the production config; every run states it so
    // the grader can snapshot what "default" meant at run time.
    println!("solari_default: {:?}", SolariLighting::default());
    // The spp-threshold dump implies --accum: it can never fire on fresh
    // frames.
    if args.dump_at_spp > 0 && !args.camera.accum {
        args.camera.accum = true;
        println!("furnace: --dump-at-spp implies --accum; accumulation enabled");
    }
    // The effective camera as RON — feed it back through `--camera` to replay
    // or hand-tweak this exact run — and its derived name (grader row identity).
    let camera = args.camera.camera();
    println!("solari_camera: {}", bevy::solari::cli::camera_ron(&camera));
    println!("solari_camera_name: {}", camera.name());
    println!("solari_scene_fp: {:016x}-{:?}", SCENE_FP, args.scene);
    if let Ok(cwd) = std::env::current_dir() {
        // The bistro scene loads assets/bistro/bistro.bsn relative to the
        // working directory (the solari_files root under xtask/grader runs).
        // SAFETY: set before any threads are spawned (AssetPlugin reads it
        // during plugin build).
        unsafe { std::env::set_var("BEVY_ASSET_ROOT", cwd) };
    }
    let mut app = App::new();
    // Exams time frames from logs: never let the unfocused 60 Hz throttle
    // (WinitSettings::game default) poison the clock.
    app.insert_resource(bevy::winit::WinitSettings {
        focused_mode: bevy::winit::UpdateMode::Continuous,
        unfocused_mode: bevy::winit::UpdateMode::Continuous,
    });
    app.insert_resource(DlssProjectId(bevy::asset::uuid::uuid!(
        "d5580e3b-691b-4fef-8dcd-58c0ae6df08e"
    )));
    app.insert_resource(OrbitSoak { frames: args.orbit, start: None });
    if let Some(secs) = args.dump_at_secs {
        app.add_systems(
            Update,
            move |time: Res<Time>, mut fd: ResMut<SolariFreezeDiff>, mut done: Local<bool>| {
                if !*done && time.elapsed_secs() >= secs {
                    *done = true;
                    fd.dump_epoch += 1;
                }
            },
        );
    }
    app.insert_resource(SolariNrc {
        inline_coopvec: args.nrc_inline,
        spread_c: args.nrc_spread_c,
        ..Default::default()
    });
    if args.fps {
        app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
    }
    if args.dlss {
        app.insert_resource(SolariDlssMode::Dlaa);
    }
    if args.no_ui {
        app.insert_resource(SolariDebugUi(false));
    }
    if let Some(secs) = args.shot_at_secs {
        let burst = args.shot_frames.max(1);
        app.add_systems(
            Update,
            move |time: Res<Time>, mut commands: Commands, mut taken: Local<u32>| {
                // One screenshot per frame once the settle window elapses,
                // until the burst is captured (consecutive frames — the
                // flicker metric's raw material).
                if *taken < burst && time.elapsed_secs() >= secs {
                    *taken += 1;
                    take_shot(&mut commands);
                }
            },
        );
    }
    if args.shot_on_dump {
        app.add_systems(Update, shot_on_dump);
    }
    app.add_systems(Update, (orbit_soak, announce_window_size));
    // The debug panels are off in graded runs — the title says what's rendering.
    let mut title = format!("Furnace — {}", camera.name());
    if args.dlss {
        title.push_str(" +DLSS");
    }
    app.add_timeout_exit(args.timeout, 10.0)
        .add_screenshot(KeyCode::F12)
        .insert_resource(SceneArgs {
            scene: args.scene,
            equal_lamps: args.lamp_law == "equal",
            camera: args.camera,
        })
        .insert_resource(SolariUniformLights { enabled: args.uniform_lights })
        .insert_resource(SolariFreezeDiff { dump_at_spp: args.dump_at_spp, ..default() })
        .add_plugins((
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title,
                        present_mode: PresentMode::AutoNoVsync,
                        // Requested render size — tiling WMs deal their own, so
                        // the grader verifies the announced size and retries.
                        resolution: WindowResolution::new(1600, 900),
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin {
                    default_sampler: ImageSamplerDescriptor {
                        address_mode_u: ImageAddressMode::Repeat,
                        address_mode_v: ImageAddressMode::Repeat,
                        address_mode_w: ImageAddressMode::Repeat,
                        ..ImageSamplerDescriptor::linear()
                    },
                    ..default()
                })
                .set(LogPlugin {
                    filter: LOG_FILTER.into(),
                    ..default()
                })
                .disable::<TransformPlugin>()
                .disable::<PbrPlugin>()
                .disable::<RenderDebugOverlayPlugin>(),
            FreeCameraPlugin,
            SolariPlugin,
            FeathersPlugins,
            util::sun::SunPlugin,
            util::park::HoverParkPlugin,
            bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
            // The bevy_city-style FPS overlay + frame-time graph. Hidden in
            // graded runs — screenshots capture the full window surface.
            bevy::dev_tools::fps_overlay::FpsOverlayPlugin {
                config: bevy::dev_tools::fps_overlay::FpsOverlayConfig {
                    enabled: !args.no_ui,
                    frame_time_graph_config: bevy::dev_tools::fps_overlay::FrameTimeGraphConfig {
                        enabled: !args.no_ui,
                        target_fps: 240.0,
                        min_fps: 60.0,
                    },
                    ..default()
                },
            },
            // F1: reflection-driven world inspector (whole-world escape hatch;
            // the per-camera card covers SolariCamera).
            bevy::feathers_inspector::WorldInspectorPlugin::new().with_toggle_key(KeyCode::F1),
        ))
        .insert_resource(UiTheme(create_dark_theme()))
        .init_resource::<Probes>()
        .add_plugins(ExtractResourcePlugin::<Probes>::default())
        .add_systems(Startup, setup_furnace.run_if(|a: Res<SceneArgs>| a.scene == Scene::Furnace))
        .add_systems(Startup, setup_room.run_if(|a: Res<SceneArgs>| a.scene == Scene::Room))
        .add_systems(Startup, setup_yard.run_if(|a: Res<SceneArgs>| a.scene == Scene::Yard))
        .add_systems(Startup, setup_bistro.run_if(|a: Res<SceneArgs>| a.scene == Scene::Bistro))
        .add_systems(Update, freeze_diff_keys);
    app.sub_app_mut(RenderApp)
        .add_systems(Render, probe_readback.in_set(RenderSystems::Cleanup));
    app.run();
}

#[derive(Resource)]
struct SceneArgs {
    scene: Scene,
    equal_lamps: bool,
    /// The shared estimator levers; `camera_mode` turns them into components.
    camera: SolariCameraArgs,
}

/// The camera the CLI levers select: the reference exam harness by default,
/// the realtime ReSTIR stack under `--camera '(mode: Realtime(()))'`.
fn camera_mode(args: &SceneArgs) -> SolariCamera {
    args.camera.camera()
}

/// A named world-space point whose surrounding pixels get averaged each probe pass.
#[derive(Clone)]
struct Probe {
    name: &'static str,
    world: DVec3,
    /// Expected luminance ratio vs the `sky` probe (furnace scene); None = log only.
    expect: Option<f64>,
}

#[derive(Resource, Clone, Default, ExtractResource)]
struct Probes(Vec<Probe>);

fn spawn_ball(
    commands: &mut Commands,
    meshes: &mut Assets<ClusterMesh>,
    materials: &mut Assets<StandardSolariMaterial>,
    sphere: &Handle<ClusterMesh>,
    name: &'static str,
    pos: DVec3,
    mat: StandardSolariMaterial,
) {
    let _ = meshes; // one shared sphere mesh; kept for signature symmetry
    commands.spawn((
        Name::new(name),
        TransformStatic,
        NoGpuGlobalTransformReadback,
        RaytracingMesh3d(sphere.clone()),
        SolariMaterial3d(materials.add(mat)),
        Transform::from_translation(pos),
    ));
}

fn setup_furnace(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<ClusterMesh>>,
    mut materials: ResMut<Assets<StandardSolariMaterial>>,
    args: Res<SceneArgs>,
) {
    // Uniform white environment: a 1×1 all-white cubemap × SKY_NITS. The whole
    // point of the furnace: incoming radiance identical from every direction.
    let mut cube = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        // f16 1.0 = 0x3C00; Rgba16Float — Rgba32Float isn't linear-filterable and
        // the raw-VK env sampler reads it as zero with no validation to say so.
        &[0x00u8, 0x3C, 0x00, 0x3C, 0x00, 0x3C, 0x00, 0x3C],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    cube.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });

    let sphere_mesh = Sphere::new(1.4)
        .mesh()
        .ico(6)
        .unwrap()
        .with_generated_tangents()
        .unwrap();
    let sphere = meshes.add(ClusterMesh::try_from(&sphere_mesh).expect("sphere bake"));

    let white = Color::WHITE;
    let x = |i: f64| DVec3::new((i - 3.5) * 3.0, 0.0, 0.0);
    let balls: [(&'static str, StandardSolariMaterial, Option<f64>); 8] = [
        (
            "diffuse_white",
            StandardSolariMaterial {
                base_color: white,
                metallic: 0.0,
                perceptual_roughness: 1.0,
                ..default()
            },
            Some(1.0),
        ),
        (
            "metal_r1.0",
            StandardSolariMaterial {
                base_color: white,
                metallic: 1.0,
                perceptual_roughness: 1.0,
                ..default()
            },
            Some(1.0),
        ),
        (
            "metal_r0.9",
            StandardSolariMaterial {
                base_color: white,
                metallic: 1.0,
                perceptual_roughness: 0.9,
                ..default()
            },
            Some(1.0),
        ),
        (
            "metal_r0.8",
            StandardSolariMaterial {
                base_color: white,
                metallic: 1.0,
                perceptual_roughness: 0.8,
                ..default()
            },
            Some(1.0),
        ),
        (
            "metal_r0.6",
            StandardSolariMaterial {
                base_color: white,
                metallic: 1.0,
                perceptual_roughness: 0.6,
                ..default()
            },
            Some(1.0),
        ),
        (
            "metal_r0.3",
            StandardSolariMaterial {
                base_color: white,
                metallic: 1.0,
                perceptual_roughness: 0.3,
                ..default()
            },
            Some(1.0),
        ),
        (
            "metal_r0.15",
            StandardSolariMaterial {
                base_color: white,
                metallic: 1.0,
                perceptual_roughness: 0.15,
                ..default()
            },
            Some(1.0),
        ),
        (
            "diffuse_grey",
            StandardSolariMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                metallic: 0.0,
                perceptual_roughness: 1.0,
                ..default()
            },
            None,
        ),
    ];
    let mut probes = vec![Probe {
        name: "sky",
        world: DVec3::new(0.0, 14.0, -14.0),
        expect: None,
    }];
    for (i, (name, mat, expect)) in balls.into_iter().enumerate() {
        let pos = x(i as f64);
        spawn_ball(
            &mut commands,
            &mut meshes,
            &mut materials,
            &sphere,
            name,
            pos,
            mat,
        );
        // Probe the sphere's screen center: facing the camera head-on, away from
        // the silhouette (where the normal-bend adaptation is deliberately biased).
        probes.push(Probe {
            name,
            world: pos + DVec3::new(0.0, 0.0, 1.4),
            expect,
        });
    }
    let cam = DVec3::new(0.0, 0.0, 42.0);

    // Rung 0.5: a lossless dielectric must also vanish (Fresnel split + TIR).
    let glass_pos = DVec3::new(-13.5, 0.0, 0.0);
    spawn_ball(
        &mut commands,
        &mut meshes,
        &mut materials,
        &sphere,
        "glass_clear",
        glass_pos,
        StandardSolariMaterial {
            specular_transmission: 1.0,
            ..default()
        },
    );
    probes.push(Probe {
        name: "glass_clear",
        world: glass_pos + DVec3::new(0.0, 0.0, 1.4),
        expect: Some(1.0),
    });

    // Sphere/parallel-slab chords stay under the critical angle, so a 45°-rotated
    // cube is what actually exercises TIR (side-face exits) — must still vanish.
    let cube_pos = DVec3::new(0.0, 7.0, 0.0);
    let cube_mesh = Cuboid::from_size(Vec3::splat(3.0))
        .mesh()
        .build()
        .with_generated_tangents()
        .unwrap();
    commands.spawn((
        Name::new("glass_tir"),
        TransformStatic,
        NoGpuGlobalTransformReadback,
        RaytracingMesh3d(meshes.add(ClusterMesh::try_from(&cube_mesh).expect("cube bake"))),
        SolariMaterial3d(materials.add(StandardSolariMaterial {
            specular_transmission: 1.0,
            ..default()
        })),
        Transform::from_translation(cube_pos)
            .with_rotation(DQuat::from_rotation_y(std::f64::consts::FRAC_PI_4)),
    ));
    probes.push(Probe {
        name: "glass_tir",
        world: cube_pos + DVec3::new(0.0, 0.0, 2.0),
        expect: Some(1.0),
    });

    // Tinted slab facing the camera: attenuation_distance = thickness makes the
    // straight-through Beer–Lambert transmittance exactly attenuation_color.
    let slab_pos = DVec3::new(0.0, -7.0, 0.0);
    let tint = [0.9, 0.6, 0.3];
    let thickness = 3.0;
    let slab_mesh = Cuboid::new(6.0, 4.0, thickness as f32)
        .mesh()
        .build()
        .with_generated_tangents()
        .unwrap();
    commands.spawn((
        Name::new("glass_tinted"),
        TransformStatic,
        NoGpuGlobalTransformReadback,
        RaytracingMesh3d(meshes.add(ClusterMesh::try_from(&slab_mesh).expect("slab bake"))),
        SolariMaterial3d(materials.add(StandardSolariMaterial {
            specular_transmission: 1.0,
            attenuation_distance: thickness as f32,
            attenuation_color: Color::linear_rgb(tint[0] as f32, tint[1] as f32, tint[2] as f32),
            ..default()
        })),
        Transform::from_translation(slab_pos).looking_at(cam, Dir3::Y),
    ));
    probes.push(Probe {
        name: "glass_tinted",
        world: slab_pos + (cam - slab_pos).normalize() * (thickness / 2.0),
        expect: Some(tinted_slab_expect(tint, 1.5)),
    });

    commands.insert_resource(Probes(probes));

    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
        Msaa::Off,
        camera_mode(&args),
        Skybox {
            image: Some(images.add(cube)),
            brightness: SKY_NITS,
            ..default()
        },
        NoGpuGlobalTransformReadback,
        FreeCamera::default(),
        Transform::from_translation(cam),
    ));
}

/// Rung-3 spatial-bias fixture: a STATIC grey room under an equal-lamp ceiling,
/// with a cluster of upright boxes + tilted ramps on the floor, plus a diffuse and
/// a metal sphere raised on pillars. The boxes cast shadows and break the floor
/// into varied normals/depths, so a spatial neighbor's domain genuinely differs
/// from its target — the M-sum bias (naive darkens at occlusion boundaries, Z-count
/// recovers) finally has something to bite; the pillared spheres add curved diffuse
/// and glossy receivers on top of that. Nothing moves, so temporal reprojection
/// stays clean (unlike the yard).
fn setup_room(
    mut commands: Commands,
    mut meshes: ResMut<Assets<ClusterMesh>>,
    mut materials: ResMut<Assets<StandardSolariMaterial>>,
    args: Res<SceneArgs>,
) {
    let grey = materials.add(StandardSolariMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        perceptual_roughness: 1.0,
        ..default()
    });
    let mut spawn = |name: String, size: Vec3, xf: Transform, mat: &Handle<StandardSolariMaterial>| {
        let mesh = Cuboid::from_size(size).mesh().build().with_generated_tangents().unwrap();
        let handle = meshes.add(ClusterMesh::try_from(&mesh).expect("cell bake"));
        commands.spawn((
            Name::new(name),
            TransformStatic,
            NoGpuGlobalTransformReadback,
            RaytracingMesh3d(handle),
            SolariMaterial3d(mat.clone()),
            xf,
        ));
    };
    let at = |p: DVec3| Transform::from_translation(p);
    // 16×6×16 grey box; floor TOP sits at y = -3.0 (slab center -3.1, half 0.1).
    let f = -3.0f64;
    spawn("floor".into(), Vec3::new(16.4, 0.2, 16.4), at(DVec3::new(0.0, -3.1, 0.0)), &grey);
    spawn("ceiling".into(), Vec3::new(16.4, 0.2, 16.4), at(DVec3::new(0.0, 3.1, 0.0)), &grey);
    spawn("wall_x-".into(), Vec3::new(0.2, 6.4, 16.4), at(DVec3::new(-8.1, 0.0, 0.0)), &grey);
    spawn("wall_x+".into(), Vec3::new(0.2, 6.4, 16.4), at(DVec3::new(8.1, 0.0, 0.0)), &grey);
    spawn("wall_z-".into(), Vec3::new(16.4, 6.4, 0.2), at(DVec3::new(0.0, 0.0, -8.1)), &grey);
    spawn("wall_z+".into(), Vec3::new(16.4, 6.4, 0.2), at(DVec3::new(0.0, 0.0, 8.1)), &grey);
    // Upright boxes (footprint, height) sitting on the floor — vertical faces +
    // flat tops (normal/depth breaks) that cast shadows (occlusion boundaries).
    let boxes = [
        (Vec3::new(1.5, 1.0, 1.5), DVec3::new(2.6, 0.0, 1.2)),
        (Vec3::new(1.0, 2.2, 1.0), DVec3::new(0.2, 0.0, 3.0)),
        (Vec3::new(2.2, 0.6, 1.1), DVec3::new(-2.2, 0.0, 2.4)),
        (Vec3::new(0.8, 1.8, 0.8), DVec3::new(3.2, 0.0, -3.0)),
        (Vec3::new(1.3, 1.4, 1.3), DVec3::new(-4.2, 0.0, 3.6)),
    ];
    for (i, (size, pos)) in boxes.iter().enumerate() {
        spawn(format!("box_{i}"), *size, at(DVec3::new(pos.x, f + size.y as f64 * 0.5, pos.z)), &grey);
    }
    // Tilted ramps — thin slabs rotated off-axis for shading-normal variety.
    spawn(
        "ramp_0".into(),
        Vec3::new(2.6, 0.15, 1.6),
        at(DVec3::new(4.2, f + 1.1, 3.2)).with_rotation(DQuat::from_rotation_z(0.6)),
        &grey,
    );
    spawn(
        "ramp_1".into(),
        Vec3::new(1.8, 0.15, 2.2),
        at(DVec3::new(-1.2, f + 0.9, -3.8)).with_rotation(DQuat::from_rotation_x(0.55)),
        &grey,
    );
    // Two spheres raised on grey pillars, side by side at the same depth: the rough
    // diffuse ball on the left, the glossy metal ball on the right — curved diffuse +
    // glossy receivers standing above the box clutter. Each ball center sits at
    // y = f + pillar_height + BALL_R; xz + height reused for the spheres + probes.
    const BALL_R: f64 = 0.7;
    let (dx, dz, d_ph) = (-1.5f64, -1.0f64, 1.8f64); // diffuse: left of the pair
    let (mx, mz, m_ph) = (0.5f64, -1.0f64, 1.8f64); //  metal: right of the pair
    for (label, x, z, ph) in [("diffuse", dx, dz, d_ph), ("metal", mx, mz, m_ph)] {
        spawn(format!("pillar_{label}"), Vec3::new(0.7, ph as f32, 0.7), at(DVec3::new(x, f + ph * 0.5, z)), &grey);
    }
    // Equal-lamp 4×4 ceiling grid (uniform lighting isolates the spatial bias from
    // power variance); `--lamp-law pilot` keeps the flood/pilot mix if wanted.
    for k in 0..16u32 {
        let (i, j) = (k % 4, k / 4);
        let power = if args.equal_lamps { 800.0 } else if k % 5 == 4 { 12000.0 } else { 2.0 };
        let lamp = materials.add(StandardSolariMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(power, power, power),
            ..default()
        });
        spawn(
            format!("lamp_{k}"),
            Vec3::new(0.9, 0.1, 0.9),
            at(DVec3::new((i as f64 - 1.5) * 3.8, 2.9, (j as f64 - 1.5) * 3.8)),
            &lamp,
        );
    }
    // Release the cuboid spawner's borrow of the asset stores so spawn_ball (which
    // needs &mut meshes/materials itself) can rest the spheres on the pillar tops.
    drop(spawn);
    let sphere_mesh = Sphere::new(BALL_R as f32).mesh().ico(6).unwrap().with_generated_tangents().unwrap();
    let sphere = meshes.add(ClusterMesh::try_from(&sphere_mesh).expect("sphere bake"));
    let diffuse_y = f + d_ph + BALL_R;
    let metal_y = f + m_ph + BALL_R;
    spawn_ball(
        &mut commands, &mut meshes, &mut materials, &sphere,
        "ball_diffuse",
        DVec3::new(dx, diffuse_y, dz),
        StandardSolariMaterial { base_color: Color::WHITE, perceptual_roughness: 1.0, ..default() },
    );
    spawn_ball(
        &mut commands, &mut meshes, &mut materials, &sphere,
        "ball_metal",
        DVec3::new(mx, metal_y, mz),
        StandardSolariMaterial { base_color: Color::WHITE, metallic: 1.0, perceptual_roughness: 0.4, ..default() },
    );
    commands.insert_resource(Probes(vec![
        Probe { name: "floor_open", world: DVec3::new(5.5, -3.0, -5.5), expect: None },
        Probe { name: "floor_amid", world: DVec3::new(-1.2, -3.0, 0.5), expect: None },
        Probe { name: "box_top", world: DVec3::new(0.2, 0.0, 3.0), expect: None },
        // Sphere fronts (camera-facing +z hemisphere), one radius off the center.
        Probe { name: "ball_diffuse", world: DVec3::new(dx, diffuse_y, dz + BALL_R), expect: None },
        Probe { name: "ball_metal", world: DVec3::new(mx, metal_y, mz + BALL_R), expect: None },
        Probe { name: "wall_z-", world: DVec3::new(0.0, 0.5, -8.0), expect: None },
    ]));
    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
        Msaa::Off,
        camera_mode(&args),
        NoGpuGlobalTransformReadback,
        FreeCamera::default(),
        Transform::from_translation(DVec3::new(3.5, 1.6, 7.0))
            .looking_at(DVec3::new(-0.5, -2.2, 0.0), Vec3::Y),
    ));
}

/// Rung-3 directional exam: an OPEN yard — sun (directional, NEE-only technique)
/// + a lamp cluster (reservoir technique) lighting the same ground. Restir mode
/// shades the sun per-light and drops the ½ stratum from the emissive pdf; if
/// either side of that bookkeeping is wrong, restir-vs-stratified means drift.
fn setup_yard(
    mut commands: Commands,
    mut meshes: ResMut<Assets<ClusterMesh>>,
    mut materials: ResMut<Assets<StandardSolariMaterial>>,
    args: Res<SceneArgs>,
) {
    let grey = materials.add(StandardSolariMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        perceptual_roughness: 1.0,
        ..default()
    });
    let mut slab = |name: String, size: Vec3, pos: DVec3, mat: &Handle<StandardSolariMaterial>| {
        let mesh = Cuboid::from_size(size).mesh().build().with_generated_tangents().unwrap();
        let handle = meshes.add(ClusterMesh::try_from(&mesh).expect("slab bake"));
        commands.spawn((
            Name::new(name),
            TransformStatic,
            NoGpuGlobalTransformReadback,
            RaytracingMesh3d(handle),
            SolariMaterial3d(mat.clone()),
            Transform::from_translation(pos),
        ));
    };
    slab("ground".into(), Vec3::new(40.0, 0.3, 40.0), DVec3::new(0.0, -0.15, 0.0), &grey);
    slab("box_a".into(), Vec3::new(2.0, 2.0, 2.0), DVec3::new(-4.0, 1.0, 0.0), &grey);
    slab("box_b".into(), Vec3::new(2.0, 2.0, 2.0), DVec3::new(0.0, 1.0, -3.0), &grey);
    slab("box_c".into(), Vec3::new(2.0, 2.0, 2.0), DVec3::new(5.0, 1.0, 2.0), &grey);
    // Lamp cluster over one corner — keeps the emissive reservoir busy while the
    // sun exercises the deterministic directional arm of the visibility loop.
    for k in 0..4u32 {
        let lamp = materials.add(StandardSolariMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(800.0, 800.0, 800.0),
            ..default()
        });
        slab(
            format!("yard_lamp_{k}"),
            Vec3::new(0.9, 0.1, 0.9),
            DVec3::new(-9.0 + (k % 2) as f64 * 2.0, 4.0, 6.0 + (k / 2) as f64 * 2.0),
            &lamp,
        );
    }
    commands.spawn((
        Name::new("sun"),
        SolariDirectionLight { illuminance: 5_000.0, ..default() },
        Transform::from_translation(DVec3::new(8.0, 16.0, 8.0)).looking_at(DVec3::ZERO, Vec3::Y),
        NoGpuGlobalTransformReadback,
    ));
    commands.insert_resource(Probes(vec![
        Probe { name: "sun_ground", world: DVec3::new(8.0, 0.0, 8.0), expect: None },
        // In box_b's shadow (sun from +x/+y/+z → shadows stretch toward -x/-z).
        Probe { name: "shadow_ground", world: DVec3::new(-1.6, 0.0, -4.8), expect: None },
        Probe { name: "lamp_ground", world: DVec3::new(-8.0, 0.0, 7.0), expect: None },
        Probe { name: "box_face", world: DVec3::new(-4.0, 1.2, 1.01), expect: None },
    ]));
    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
        Msaa::Off,
        camera_mode(&args),
        NoGpuGlobalTransformReadback,
        FreeCamera::default(),
        Transform::from_translation(DVec3::new(2.0, 7.0, 20.0))
            .looking_at(DVec3::new(-1.0, 0.0, 0.0), Vec3::Y),
    ));
}


/// Real-content exam: the importer-baked bistro `.bsn` (glass, foliage
/// cutouts, dense geometry) under sun + atmosphere. No probes — this scene is
/// graded purely by FLIP vs its own truth.
fn setup_bistro(mut commands: Commands, asset_server: Res<AssetServer>, args: Res<SceneArgs>) {
    // Entities arrive carrying `RaytracingMesh3d` + inline `SolariMaterial`;
    // the root supplies the Transform. No Visibility anywhere — that's a
    // raster concept; RT extraction ignores it and hiding an instance is
    // removing its `RaytracingMesh3d` (or a `RenderLayers` cull-mask edit).
    commands.spawn((
        Name::new("bistro"),
        ScenePatchInstance(asset_server.load("bistro/bistro.bsn".to_string())),
        Transform::default(),
    ));
    // Sun: steered by the inspector card ([`util::sun::SunSettings`] stamps
    // direction + illuminance on spawn); the warm color is scene-owned.
    commands.spawn((
        Name::new("sun"),
        util::sun::Sun,
        Transform::default(),
        SolariDirectionLight {
            color: Color::srgb(1.0, 0.87, 0.78),
            ..default()
        },
    ));
    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
        Msaa::Off,
        camera_mode(&args),
        SolariAtmosphere::default(),
        NoGpuGlobalTransformReadback,
        FreeCamera::default(),
        Transform::from_translation(DVec3::new(-10.0, 2.0, 0.0))
            .looking_at(DVec3::new(0.0, 0.0, 0.0), Vec3::Y),
    ));
}

/// F = freeze reference snapshot, V = toggle |current-frozen| diff view,
/// P = dump EXR (estimator edits live in the camera inspector panel).
fn freeze_diff_keys(input: Res<ButtonInput<KeyCode>>, mut fd: ResMut<SolariFreezeDiff>) {
    if input.just_pressed(KeyCode::KeyF) {
        fd.freeze_epoch += 1;
    }
    if input.just_pressed(KeyCode::KeyV) {
        fd.diff = !fd.diff;
        info!("diff view: {}", fd.diff);
    }
    if input.just_pressed(KeyCode::KeyP) {
        fd.dump_epoch += 1;
    }
}

fn luminance(c: [f32; 3]) -> f64 {
    0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64
}

/// Normal-incidence slab under a uniform sky: every interreflection order escapes
/// to the same sky, so E = R + (1−R)²·a/(1−R·a) per channel; luminance-weighted.
fn tinted_slab_expect(a: [f64; 3], ior: f64) -> f64 {
    let r = ((ior - 1.0) / (ior + 1.0)).powi(2);
    let e = a.map(|a| r + (1.0 - r).powi(2) * a / (1.0 - r * a));
    luminance([e[0] as f32, e[1] as f32, e[2] as f32])
}

/// Render-world probe: every ~2s copy the RT output buffer (the accumulated mean)
/// to a staging buffer, average a 9×9 patch around each probe's projected pixel,
/// and log means + ratios. PASS/FAIL on `expect` ratios once past 4096 spp.
fn probe_readback(
    views: Query<(
        &RtOutputBuffer,
        Option<&RtAccumulation>,
        &ExtractedCamera,
        &ExtractedView,
    )>,
    probes: Res<Probes>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut staging: Local<Option<Buffer>>,
    mut tick: Local<u32>,
) {
    *tick += 1;
    if *tick % 240 != 0 || probes.0.is_empty() {
        return;
    }
    for (output, accumulation, camera, view) in &views {
        let Some(viewport) = camera.physical_viewport_size else {
            continue;
        };
        let spp = accumulation.map_or(0, |a| a.n);

        let staging = staging.get_or_insert_with(|| {
            render_device.create_buffer(&BufferDescriptor {
                label: Some("furnace_probe_staging"),
                size: output.size,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        if staging.size() != output.size {
            *staging = render_device.create_buffer(&BufferDescriptor {
                label: Some("furnace_probe_staging"),
                size: output.size,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("furnace_probe_copy"),
        });
        encoder.copy_buffer_to_buffer(&output.buffer, 0, staging, 0, output.size);
        render_queue.submit([encoder.finish()]);

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = render_device.poll(PollType::wait_indefinitely());
        if rx.recv().map(|r| r.is_err()).unwrap_or(true) {
            return;
        }
        let pixels: Vec<[f32; 4]> = slice
            .get_mapped_range()
            .chunks_exact(16)
            .map(|c| [0, 4, 8, 12].map(|o| f32::from_le_bytes(c[o..o + 4].try_into().unwrap())))
            .collect();
        staging.unmap();

        // Project each probe through the view (camera-relative, matching the trace).
        let inv_view = view.world_from_view.affine().inverse();
        let mut means: Vec<(usize, f64, [f32; 3])> = Vec::new();
        for (pi, probe) in probes.0.iter().enumerate() {
            let vpos = inv_view.transform_point3(probe.world.as_vec3());
            let clip = view.clip_from_view * Vec4::new(vpos.x, vpos.y, vpos.z, 1.0);
            if clip.w <= 0.0 {
                continue;
            }
            let ndc = clip.truncate() / clip.w;
            let px = ((ndc.x * 0.5 + 0.5) * viewport.x as f32) as i64;
            let py = ((-ndc.y * 0.5 + 0.5) * viewport.y as f32) as i64;
            let mut sum = [0.0f64; 3];
            let mut count = 0u32;
            for dy in -4i64..=4 {
                for dx in -4i64..=4 {
                    let (x, y) = (px + dx, py + dy);
                    if x < 0 || y < 0 || x >= viewport.x as i64 || y >= viewport.y as i64 {
                        continue;
                    }
                    let p = pixels[(y as u32 * viewport.x + x as u32) as usize];
                    sum[0] += p[0] as f64;
                    sum[1] += p[1] as f64;
                    sum[2] += p[2] as f64;
                    count += 1;
                }
            }
            if count > 0 {
                let m = [
                    (sum[0] / count as f64) as f32,
                    (sum[1] / count as f64) as f32,
                    (sum[2] / count as f64) as f32,
                ];
                means.push((pi, luminance(m), m));
            }
        }

        let sky = means
            .iter()
            .find(|(pi, ..)| probes.0[*pi].name == "sky")
            .map(|&(_, lum, _)| lum);
        let mut line = format!("probes @ {spp} spp:");
        let mut failures = 0u32;
        for (pi, lum, m) in &means {
            let probe = &probes.0[*pi];
            match (sky, probe.expect) {
                (Some(sky), Some(expect)) if sky > 0.0 => {
                    let ratio = lum / sky;
                    let verdict = if spp >= 4096 {
                        failures += ((ratio - expect).abs() > 0.01) as u32;
                        if (ratio - expect).abs() > 0.01 {
                            " FAIL"
                        } else {
                            " ok"
                        }
                    } else {
                        ""
                    };
                    line += &format!(" | {} r={ratio:.4}{verdict}", probe.name);
                }
                _ => {
                    line += &format!(
                        " | {} mean=({:.4},{:.4},{:.4})",
                        probe.name, m[0], m[1], m[2]
                    );
                }
            }
        }
        info!("{line}");
        if spp >= 4096 {
            if failures == 0 {
                info!("furnace: PASS ({} probes)", means.len());
            } else {
                warn!("furnace: {failures} probe(s) FAILED");
            }
        }
    }
}

// ---- Exam-harness plumbing (self-contained; no deps outside bevy) ----

/// Log filter: silence noisy startup INFO lines (keep warn/error).
use util::LOG_FILTER;

#[derive(Resource)]
struct Timeout(Timer);

/// Screenshot the primary window into `target/tmp/` and print the
/// machine-readable line the grader parses once the PNG is on disk. This
/// captures the FINAL surface — post-DLSS-RR, post-tonemap, UI included
/// (graded runs pass --no-ui).
fn take_shot(commands: &mut Commands) {
    let dir = std::path::Path::new("./target/tmp");
    let _ = std::fs::create_dir_all(dir);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("solari-shot-{ms}.png"));
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            let img = captured.image.clone();
            match img.try_into_dynamic() {
                // Alpha carries HDR brightness values — drop it (rgb8) or the
                // PNG comes out wrong, same as bevy's save_to_disk.
                Ok(dyn_img) => match dyn_img.to_rgb8().save(&path) {
                    Ok(()) => info!("solari shot: wrote {}", path.display()),
                    Err(e) => error!("solari shot: {e}"),
                },
                Err(e) => error!("solari shot: {e}"),
            }
        },
    );
}

/// With `--shot-on-dump`: when the `--dump-at-spp` EXR lands in `target/tmp`,
/// take the display-referred twin screenshot (the truth bake's PNG reference
/// for the per-frame protocol). The spp counter lives in the render world, so
/// the dump file appearing IS the signal — polled twice a second; the extra
/// accumulation frames between dump and shot only converge the truth further.
fn shot_on_dump(
    mut commands: Commands,
    time: Res<Time>,
    mut state: Local<(f32, bool)>,
) {
    let (next_poll, done) = &mut *state;
    if *done || time.elapsed_secs() < *next_poll {
        return;
    }
    *next_poll = time.elapsed_secs() + 0.5;
    let fresh_dump = std::fs::read_dir("target/tmp")
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "exr")
                && entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age.as_secs_f32() < 2.0)
        });
    if fresh_dump {
        *done = true;
        take_shot(&mut commands);
    }
}

trait ExamAppExt {
    fn add_timeout_exit(&mut self, duration: Option<f32>, agent_default: f32) -> &mut Self;
    fn add_screenshot(&mut self, trigger: KeyCode) -> &mut Self;
}

impl ExamAppExt for App {
    /// Bounded runs: `--timeout N`, else `CLAUDECODE` (agent/CI) defaults to
    /// `agent_default` seconds; interactive runs never time out.
    fn add_timeout_exit(&mut self, duration: Option<f32>, agent_default: f32) -> &mut Self {
        let secs = duration.or_else(|| std::env::var_os("CLAUDECODE").map(|_| agent_default));
        if let Some(secs) = secs {
            self.insert_resource(Timeout(Timer::from_seconds(secs, TimerMode::Once)))
                .add_systems(
                    Update,
                    |time: Res<Time>, mut t: ResMut<Timeout>, mut exit: MessageWriter<AppExit>| {
                        if t.0.tick(time.delta()).just_finished() {
                            info!("timeout reached, exiting");
                            exit.write(AppExit::Success);
                        }
                    },
                );
        }
        self
    }

    /// Manual screenshots on `trigger`; headless/agent runs (no keyboard) can set
    /// `AUTO_SCREENSHOT_MS=8000` to capture once that long after startup.
    fn add_screenshot(&mut self, trigger: KeyCode) -> &mut Self {
        fn take(commands: &mut Commands) {
            let dir = std::path::Path::new("./target/tmp");
            let _ = std::fs::create_dir_all(dir);
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let path = dir.join(format!("screenshot-{ms}.png"));
            info!("Screenshot saving to {}", path.display());
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        }
        self.add_systems(
            Update,
            move |mut commands: Commands, input: Res<ButtonInput<KeyCode>>| {
                if input.just_pressed(trigger) {
                    take(&mut commands);
                }
            },
        )
        .add_systems(
            Update,
            |mut commands: Commands, time: Res<Time>, mut taken: Local<bool>| {
                if *taken {
                    return;
                }
                let Some(ms) = std::env::var("AUTO_SCREENSHOT_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                else {
                    *taken = true; // not requested — check the env var only once
                    return;
                };
                if time.elapsed_secs_f64() * 1000.0 >= ms as f64 {
                    *taken = true;
                    take(&mut commands);
                }
            },
        )
    }
}

/// Motion soak: sway + yaw around the spawn pose, snap back after `frames`.
#[derive(Resource)]
struct OrbitSoak {
    frames: u32,
    start: Option<Transform>,
}

/// Announce the primary window's physical size whenever it changes — the dump
/// resolution follows it, and the grader compares this line against its cached
/// truth's dims to abort-and-retry a wrongly-tiled launch early.
fn announce_window_size(
    mut last: Local<(u32, u32)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if let Ok(window) = windows.single() {
        let size = (window.physical_width(), window.physical_height());
        if *last != size && size.0 > 0 {
            *last = size;
            info!("solari window: {}x{}", size.0, size.1);
        }
    }
}

fn orbit_soak(
    frames: Res<bevy::diagnostic::FrameCount>,
    mut orbit: ResMut<OrbitSoak>,
    mut cams: Query<&mut Transform, With<SolariCamera>>,
) {
    if orbit.frames == 0 {
        return;
    }
    let Ok(mut tf) = cams.single_mut() else {
        return;
    };
    if orbit.start.is_none() {
        orbit.start = Some(*tf);
    }
    let start = orbit.start.unwrap();
    // Frame-indexed: pose is a pure function of the frame number, so frame k
    // is the same pose at any frame rate — motion captures compare equal
    // across recipes. (The old time-based sway made faster recipes sweep less
    // per frame.) Rates assume a nominal 60 fps worth of the old feel.
    let f = frames.0;
    if f >= orbit.frames + 60 {
        *tf = start;
        return;
    }
    // Settle phase: near-final pose while motion-contaminated history heals
    // (m-cap frames), then the exact pose forces one clean accumulation restart.
    if f >= orbit.frames {
        *tf = start;
        tf.translation += start.rotation * DVec3::new(1.0e-4, 0.0, 0.0);
        return;
    }
    let t = f as f64 / 60.0;
    let sway = (t * 0.9).sin() * 1.0;
    let bob = (t * 1.3).sin() * 0.25;
    *tf = start;
    let offset = start.rotation * DVec3::new(sway, bob, 0.0);
    tf.translation += offset;
    tf.rotate_y((t * 0.55).sin() * 0.15);
}
