//! Accuracy × speed grader for the solari estimator recipes.
//!
//! Two axes, one table:
//! - **Accuracy**: each recipe accumulates for the same wall-clock time on a
//!   static furnace-scene camera, dumps the radiance buffer, and is scored
//!   against a cached converged truth with NVIDIA HDR-FLIP (full-image
//!   perceptual error — not a handful of probe pixels).
//! - **Speed**: the median per-frame time over the same run (furnace `--fps`
//!   diagnostics). For perf under camera motion use `solari_view --bench`.
//!
//! The truth is a plain reference run accumulated to `--truth-spp`, keyed by
//! the config's transport scope (full / no-GI / GI-only — a `gi: None` config
//! is graded against a DI-only truth, not the full image) and cached under
//! `target/solari_grader/<scene>/` so each bakes once.
//!
//! Dumps happen pre-DLSS-RR (the RT output buffer), so the default recipe is
//! graded on its raw estimator output; RR's temporal polish is a separate
//! question (flicker grading is a follow-up).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use clap::Parser;

/// The estimator ladder: every rung as a complete `SolariCamera` RON — the
/// config IS the identity. Rows, files, and report labels use the name furnace
/// derives from it (`solari_camera_name:`), so the table always tells the
/// truth about what was graded.
const LADDER: &[&str] = &[
    // Plain reference accumulator (NEE-1, path-traced GI).
    "(mode: Reference(()))",
    // ReSTIR DI, temporal only (payload structs default to the canonical levers).
    "(mode: Reference((di: Restir(()))))",
    // ReSTIR DI + the spatial merge+shade pass.
    "(mode: Reference((di: Restir((spatial: Some(()))))))",
    // ReSTIR DI + GI reconnection reservoirs (recon + temporal). `gi` is an
    // Option<GiArm> and the ReSTIR selector lives at GiArm.estimator, so the
    // arm is `Some((estimator: Restir(())))` — a bare `gi: Restir(())` fails
    // to parse as SolariCamera RON (furnace panics, the rung is skipped).
    "(mode: Reference((di: Restir(()), gi: Some((estimator: Restir(()))))))",
    // Spatial reuse on both arms.
    "(mode: Reference((di: Restir((spatial: Some(()))), gi: Some((estimator: Restir((spatial: Some(()))))))))",
    // Reference with NRC GI termination — the cache-bias A/B.
    "(mode: Reference((gi: Some((nrc: true)))))",
    // The stack exactly as shipped: SolariLighting::default().
    "(mode: Realtime(()))",
];

#[derive(Parser)]
#[command(about = "Grade solari recipes on accuracy (HDR-FLIP vs converged truth) and speed")]
struct Args {
    /// furnace scene to grade on (furnace | room | yard | bistro)
    #[arg(long, default_value = "room")]
    scene: String,

    /// grade only ladder rungs whose RON contains this substring (e.g.
    /// "Realtime", "nrc: true", "spatial: Some")
    #[arg(long)]
    only: Option<String>,

    /// seconds each recipe runs before its dump — per-frame: the settle
    /// window (temporal chains + NRC maturity); --accum: the equal-time
    /// accumulation budget
    #[arg(long, default_value_t = 20.0)]
    time: f32,

    /// grade accumulated convergence exams (the correctness protocol,
    /// pre-RR EXR + HDR-FLIP) instead of the default per-frame protocol,
    /// which screenshots one fresh 1-spp frame through DLSS-RR and the
    /// display transform after the settle window — what the user sees
    #[arg(long)]
    accum: bool,

    /// per-frame protocol only: grade the raw estimator without DLSS-RR
    /// (the with/without denoiser A/B)
    #[arg(long)]
    no_dlss: bool,

    /// per-frame protocol: consecutive frames captured per run. Pairwise FLIP
    /// between neighbors is the FLICKER column — temporally-sticky error DLSS
    /// preserves scores low there (and high vs truth); per-frame flux scores
    /// high. The first frame is the render/FLIP-vs-truth candidate
    #[arg(long, default_value_t = 4)]
    shots: u32,

    /// grade under camera motion: forwards `--orbit N` (frame-indexed sway,
    /// pose is a function of the frame NUMBER so motion compares equal across
    /// recipes at any frame rate). FLIP-vs-truth is skipped — the camera isn't
    /// at the truth pose — so motion runs grade on flicker + frame time
    #[arg(long)]
    orbit: Option<u32>,

    /// extra settle seconds for NRC-bearing recipes (nrc, default) before a
    /// per-frame dump: the cache trains from random weights every launch, so
    /// steady-state quality needs the weights matured, not just the bit-20
    /// gate open
    #[arg(long, default_value_t = 40.0)]
    nrc_warmup: f32,

    /// truth convergence target in samples per pixel
    #[arg(long, default_value_t = 8192)]
    truth_spp: u32,

    /// rebake the truth even if a cached one exists
    #[arg(long)]
    rebake_truth: bool,

    /// bake/refresh the truth cache and exit (no recipes graded, no run
    /// recorded) — pre-bake expensive truths on your schedule
    #[arg(long)]
    truth_only: bool,

    /// path to the FLIP CLI binary (or set FLIP_BIN)
    #[arg(long)]
    flip: Option<PathBuf>,


    /// label recorded in the run's JSON (e.g. "baseline", "nrc-anchor-fix") —
    /// the report's compare view diffs labeled runs
    #[arg(long, default_value = "")]
    label: String,

    /// grade one custom config instead of the recipe sweep: a complete
    /// SolariCamera as RON, passed to furnace verbatim (every run prints its
    /// own as `solari_camera:` — copy, tweak, regrade). Pair with --label
    #[arg(long)]
    camera: Option<String>,

    /// extra furnace args appended to every graded run (harness levers like
    /// --lamp-law; estimator A/Bs go through --camera); pair with --label
    #[arg(long, allow_hyphen_values = true)]
    extra: Vec<String>,

}

fn main() {
    let args = Args::parse();
    let flip = args
        .flip
        .clone()
        .or_else(|| std::env::var_os("FLIP_BIN").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/mnt/code/f/flip/src/cpp/build/tool/flip"));
    if !flip.is_file() {
        eprintln!(
            "FLIP CLI not found at {} — build it (cmake -B build && cmake --build build, in <flip>/src/cpp) or pass --flip",
            flip.display()
        );
        std::process::exit(1);
    }

    let out_dir = PathBuf::from(format!("target/solari_grader/{}", args.scene));
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let configs: Vec<&str> = match &args.camera {
        Some(ron) => vec![ron.as_str()],
        None => LADDER
            .iter()
            .copied()
            .filter(|ron| args.only.as_ref().is_none_or(|f| ron.contains(f.as_str())))
            .collect(),
    };
    if configs.is_empty() {
        eprintln!("no ladder rung matches --only {:?}", args.only);
        std::process::exit(1);
    }

    // Truths: one per transport scope among the graded configs, baked on
    // first need and cached in the scene dir.
    let mut truths = std::collections::BTreeMap::new();
    for config in &configs {
        let scope = scope_of(config);
        if !truths.contains_key(&scope) {
            truths.insert(scope, ensure_truth(&args, &out_dir, scope));
        }
    }
    if args.truth_only {
        return;
    }

    // Candidates: one capture per recipe after --time seconds, then FLIP vs
    // truth. The protocol is pinned here, not inherited from CLI defaults:
    // per-frame = one fresh 1-spp frame once temporal chains and caches have
    // settled, captured as a SCREENSHOT — post-DLSS-RR, post-tonemap, what the
    // user sees — graded LDR-FLIP vs the truth's shot twin; --accum =
    // equal-time accumulation at 16 spp/frame dumped pre-RR as EXR and graded
    // HDR-FLIP (the convergence exam, exposure-independent precision).
    let mut rows = Vec::new();
    let mut default_config = String::new();
    for config in &configs {
        // NRC configs need their per-frame settle extended: the cache's
        // steady state is a training outcome, not a frame count. Realtime
        // carries NRC via its own default.
        let flat = config.replace(' ', "");
        let uses_nrc = flat.contains("nrc:true")
            || flat.contains("nrc_gi:true")
            || flat.contains("Realtime(");
        let dump_secs = if !args.accum && uses_nrc {
            args.time + args.nrc_warmup
        } else {
            args.time
        };
        println!("grading: {config} ({dump_secs}s)");
        // The RON carries the estimator identity; the protocol owns the
        // session knobs (spp/accumulation) and the capture machinery.
        let mut furnace_args: Vec<String> =
            vec!["--camera".into(), (*config).into(), "--no-ui".into()];
        if args.accum {
            furnace_args.extend([
                "--accum".into(),
                "--spp".into(),
                "16".into(),
                "--dump-at-secs".into(),
                dump_secs.to_string(),
            ]);
        } else {
            furnace_args.extend([
                "--spp".into(),
                "1".into(),
                "--shot-at-secs".into(),
                dump_secs.to_string(),
                "--shot-frames".into(),
                args.shots.max(1).to_string(),
            ]);
            if !args.no_dlss {
                furnace_args.push("--dlss".into());
            }
            if let Some(orbit) = args.orbit {
                furnace_args.extend(["--orbit".into(), orbit.to_string()]);
            }
        }
        furnace_args.push("--fps".into());
        furnace_args.extend(args.extra.iter().cloned());
        let truth = &truths[&scope_of(config)];
        // A wrongly-tiled window is detected from the size announcement and
        // aborted within seconds — relaunch a few times before giving up.
        let expect = (!truth.dims.is_empty()).then_some(truth.dims.as_str());
        let mut run = run_furnace(&args, &furnace_args, dump_secs + 4.0, false, expect);
        for attempt in 2..=3 {
            if !run.wrong_size {
                break;
            }
            println!(
                "  retrying ({attempt}/3) — free the workspace so the window tiles at {}",
                truth.dims
            );
            run = run_furnace(&args, &furnace_args, dump_secs + 4.0, false, expect);
        }
        if default_config.is_empty()
            && let Some(config) = &run.default_config
        {
            default_config = config.clone();
        }
        // Stale-truth guard: the scene source changed since the cached truth
        // was baked — every FLIP would measure "different scene", not the
        // estimator (dims can't catch content drift). Abort loudly.
        if let Some(fp) = &run.scene_fp
            && !truth.fp.is_empty()
            && *fp != truth.fp
        {
            eprintln!(
                "scene changed since the truth was baked (fingerprint {fp} vs cached {}) \
                 — rerun with --rebake-truth",
                truth.fp
            );
            std::process::exit(1);
        }
        // Row identity: the name furnace derived FROM the config.
        let name = run
            .camera_name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());
        // FLIP needs identical resolutions; the window size decides the
        // capture's. A mismatch means the truth was baked at a different
        // window size.
        let render = format!("{name}.render.png");
        let mut flicker = None;
        let (flip_mean, spp) = if args.accum {
            let Some(dump) = run.dump else {
                eprintln!("  {name}: no dump produced — skipped");
                continue;
            };
            let dims = dump_dims(&dump).unwrap_or_default();
            if !truth.dims.is_empty() && dims != truth.dims {
                eprintln!(
                    "  {name}: dump is {dims} but the truth is {} — keep the \
                     window size consistent (or --rebake-truth); skipped",
                    truth.dims
                );
                continue;
            }
            let exr = out_dir.join(format!("{name}.exr"));
            std::fs::copy(&dump, &exr).expect("copy candidate exr");
            write_render_png(&out_dir.join(&render), &read_exr(&exr), truth.exposure);
            (run_flip(&flip, &truth.exr, &exr, &out_dir, &name), run.dump_spp)
        } else {
            let Some(shot) = run.shots.first() else {
                eprintln!("  {name}: no screenshot produced — skipped");
                continue;
            };
            let dims = png_dims(shot).unwrap_or_default();
            if !truth.dims.is_empty() && dims != truth.dims {
                eprintln!(
                    "  {name}: shot is {dims} but the truth is {} — keep the \
                     window size consistent (or --rebake-truth); skipped",
                    truth.dims
                );
                continue;
            }
            // The first screenshot IS the render — PNG in, LDR-FLIP.
            std::fs::copy(shot, out_dir.join(&render)).expect("copy candidate shot");
            // Flicker: mean pairwise FLIP between CONSECUTIVE burst frames —
            // truth-free, so it survives motion runs.
            let pair_scores: Vec<f64> = run
                .shots
                .windows(2)
                .filter_map(|pair| flip_mean_only(&flip, &pair[0], &pair[1]))
                .collect();
            if !pair_scores.is_empty() {
                flicker = Some(pair_scores.iter().sum::<f64>() / pair_scores.len() as f64);
            }
            // Under motion the camera isn't at the truth pose — a truth FLIP
            // would grade "different view", not the estimator.
            let vs_truth = if args.orbit.is_some() {
                None
            } else {
                run_flip(&flip, &truth.shot, &out_dir.join(&render), &out_dir, &name)
            };
            (vs_truth, None)
        };
        rows.push(Row {
            recipe: name.clone(),
            flip: flip_mean,
            frame_ms: median(&run.frame_ms),
            spp,
            map: Some(format!("{name}.png")),
            render: Some(render),
            camera: run.camera_ron,
            flicker,
        });
    }

    // The run JSON records one truth image; runs are single-scope in practice
    // (the ladder is all full-transport, --camera is one config), so pick the
    // full truth when present, else the only other scope graded.
    let truth_repr = truths
        .get(&Scope::Full)
        .or_else(|| truths.values().next())
        .expect("at least one truth");
    report(&args, &out_dir, &mut rows, &default_config, truth_repr);

    // Rebuild the HTML report so it always reflects the latest run, and print
    // an openable link (the script scans every scene's recorded runs).
    let script = Path::new("scripts/grade_report.py");
    if script.is_file() {
        let ok = Command::new("python3")
            .arg(script)
            .status()
            .is_ok_and(|s| s.success());
        if ok && let Ok(abs) = std::fs::canonicalize("target/solari_grader/report.html") {
            let url = format!("file://{}", abs.display());
            println!("open: {url}");
            // Open in the default browser directly — terminal link handlers
            // (VS Code's) route file:// links to the editor instead.
            // SOLARI_GRADER_NO_OPEN suppresses it for batch sweeps (one tab
            // at the end beats one per scene).
            if std::env::var_os("SOLARI_GRADER_NO_OPEN").is_none() {
                let _ = Command::new("xdg-open").arg(&url).spawn();
            }
        }
    } else {
        eprintln!("html report skipped: {} not found (run from the workspace root)", script.display());
    }
}

/// The light-transport scope of a config — which converged truth it can be
/// graded against. Estimator choices (NEE / ReSTIR / NRC / RIS M) all
/// converge to the same answer; the gi arm's shape changes WHAT is rendered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Full,
    NoGi,
    GiOnly,
}

/// Scope from the config RON. String-level, like the NRC-warmup sniff: the
/// grader treats configs as opaque strings and furnace owns the parse.
fn scope_of(config: &str) -> Scope {
    let flat = config.replace(' ', "");
    if flat.contains("gi:None") {
        Scope::NoGi
    } else if flat.contains("only:true") {
        Scope::GiOnly
    } else {
        Scope::Full
    }
}

impl Scope {
    /// The truth camera: plain reference accumulation at matching scope.
    /// `bounces: 32` is PINNED — the shipped default is 1 (the RR-facing
    /// production shape), but a truth must converge the full transport.
    fn truth_camera(self) -> &'static str {
        match self {
            Scope::Full => "(mode: Reference((gi: Some((bounces: 32)))))",
            Scope::NoGi => "(mode: Reference((gi: None)))",
            Scope::GiOnly => "(mode: Reference((gi: Some((only: true, bounces: 32)))))",
        }
    }

    /// Truth filename prefix in the scene dir; full transport is the common
    /// case and keeps the plain `truth` name.
    fn prefix(self) -> &'static str {
        match self {
            Scope::Full => "truth",
            Scope::NoGi => "truth-nogi",
            Scope::GiOnly => "truth-gionly",
        }
    }
}

/// A resolved truth cache entry for one scope.
struct Truth {
    prefix: &'static str,
    /// HDR-FLIP reference for exams.
    exr: PathBuf,
    /// Screenshot twin — LDR-FLIP reference for per-frame runs (the same
    /// converged image through the same display transform the candidates
    /// ship through).
    shot: PathBuf,
    /// `WxH` of the truth capture — enforced on every candidate.
    dims: String,
    /// Scene fingerprint at bake time — the stale-truth guard.
    fp: String,
    /// Photographic exposure derived from the EXR; every render PNG graded
    /// against this truth is exposed identically, so they compare fairly.
    exposure: f32,
}

/// Return the scope's truth, baking it first when the cache is missing.
fn ensure_truth(args: &Args, out_dir: &Path, scope: Scope) -> Truth {
    let prefix = scope.prefix();
    let exr = out_dir.join(format!("{prefix}-{}spp.exr", args.truth_spp));
    let shot = out_dir.join(format!("{prefix}-{}spp.shot.png", args.truth_spp));
    let dims_path = out_dir.join(format!("{prefix}.dims"));
    let fp_path = out_dir.join(format!("{prefix}.fp"));
    // The dims sidecar doubles as the cache-format marker: caches without it
    // (or without the shot twin) can't be graded — rebake.
    let valid = exr.is_file() && shot.is_file() && dims_path.is_file();
    if args.rebake_truth || !valid {
        println!(
            "baking truth: {} {} @ {} spp (cached at {})",
            args.scene,
            scope.truth_camera(),
            args.truth_spp,
            exr.display()
        );
        let run = run_furnace(
            args,
            &[
                "--camera".into(),
                scope.truth_camera().into(),
                "--accum".into(),
                "--spp".into(),
                "32".into(),
                "--dump-at-spp".into(),
                args.truth_spp.to_string(),
                "--shot-on-dump".into(),
                "--no-ui".into(),
            ],
            // Truth bakes until the spp threshold fires the dump; generous cap.
            // The truth DEFINES the dims, so no expected size to enforce.
            3600.0,
            true,
            None,
        );
        let dump = run.dump.expect("truth run produced no dump — see its output above");
        std::fs::copy(&dump, &exr).expect("cache truth exr");
        let capture = run
            .shots
            .first()
            .expect("truth run produced no screenshot — see its output above");
        std::fs::copy(capture, &shot).expect("cache truth shot");
        std::fs::write(&dims_path, dump_dims(&dump).unwrap_or_default())
            .expect("record truth dims");
        std::fs::write(&fp_path, run.scene_fp.unwrap_or_default())
            .expect("record truth scene fingerprint");
    } else {
        println!("truth: cached {}", exr.display());
    }
    let pixels = read_exr(&exr);
    let exposure = auto_exposure(&pixels.2);
    let render = out_dir.join(format!("{prefix}-{}spp.render.png", args.truth_spp));
    if args.rebake_truth || !render.is_file() {
        write_render_png(&render, &pixels, exposure);
    }
    Truth {
        prefix,
        exr,
        shot,
        dims: std::fs::read_to_string(&dims_path).unwrap_or_default(),
        fp: std::fs::read_to_string(&fp_path).unwrap_or_default(),
        exposure,
    }
}

struct Row {
    recipe: String,
    flip: Option<f64>,
    frame_ms: Option<f64>,
    spp: Option<u32>,
    /// FLIP error-map filename in the scene dir.
    map: Option<String>,
    /// Tonemapped render filename in the scene dir.
    render: Option<String>,
    /// The exact `SolariCamera` graded, as replayable RON.
    camera: Option<String>,
    /// Mean consecutive-frame FLIP over the shot burst (per-frame protocol) —
    /// the flicker score. Low + high-vs-truth = sticky error DLSS preserves.
    flicker: Option<f64>,
}

/// Sort by FLIP (best first), print the table, and write the CSV.
fn report(args: &Args, out_dir: &Path, rows: &mut [Row], default_config: &str, truth: &Truth) {
    // Best first: FLIP when the run has it, flicker otherwise (motion runs).
    rows.sort_by(|a, b| {
        a.flip
            .or(a.flicker)
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.flip.or(b.flicker).unwrap_or(f64::INFINITY))
    });
    println!();
    let protocol = match (args.accum, args.no_dlss, args.orbit.is_some()) {
        (true, ..) => "equal-time exam",
        (false, true, false) => "per-frame",
        (false, true, true) => "per-frame+motion",
        (false, false, false) => "per-frame+dlss",
        (false, false, true) => "per-frame+dlss+motion",
    };
    println!(
        "scene {} | {}s {protocol} | truth {} spp | FLIP·ms = error × cost (lower is better on every column)",
        args.scene, args.time, args.truth_spp
    );
    println!(
        "{:<20} {:>8} {:>9} {:>9} {:>9} {:>7}",
        "config", "FLIP", "flicker", "avg ms", "FLIP·ms", "spp"
    );
    let mut csv = String::from("config,flip_mean,flicker,avg_frame_ms,flip_x_ms,spp\n");
    for row in rows.iter() {
        let flip = row.flip.map_or("-".into(), |v| format!("{v:.4}"));
        let ms = row.frame_ms.map_or("-".into(), |v| format!("{v:.2}"));
        let product = row
            .flip
            .zip(row.frame_ms)
            .map_or("-".into(), |(f, m)| format!("{:.3}", f * m));
        let spp = row.spp.map_or("-".into(), |v| v.to_string());
        let flicker = row.flicker.map_or("-".into(), |v| format!("{v:.4}"));
        println!(
            "{:<20} {:>8} {:>9} {:>9} {:>9} {:>7}",
            row.recipe, flip, flicker, ms, product, spp
        );
        csv.push_str(&format!(
            "{},{},{},{},{},{}\n",
            row.recipe,
            row.flip.map_or(String::new(), |v| v.to_string()),
            row.flicker.map_or(String::new(), |v| v.to_string()),
            row.frame_ms.map_or(String::new(), |v| v.to_string()),
            row.flip
                .zip(row.frame_ms)
                .map_or(String::new(), |(f, m)| (f * m).to_string()),
            row.spp.map_or(String::new(), |v| v.to_string()),
        ));
    }
    let csv_path = out_dir.join("report.csv");
    std::fs::write(&csv_path, csv).expect("write report csv");
    println!("\nreport: {}", csv_path.display());

    // Machine record for `scripts/grade_report.py`: one JSON per run, so the
    // report can plot history and diff labeled runs critcmp-style.
    let runs_dir = out_dir.join("runs");
    let _ = std::fs::create_dir_all(&runs_dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = if args.label.is_empty() {
        format!("{stamp}.json")
    } else {
        format!("{stamp}-{}.json", args.label.replace(['/', ' '], "_"))
    };
    // Per-frame runs show the truth's screenshot twin (same display transform
    // as their candidates); exams show the ACES render of the truth EXR.
    let truth_render_name = if args.accum {
        format!("{}-{}spp.render.png", truth.prefix, args.truth_spp)
    } else {
        format!("{}-{}spp.shot.png", truth.prefix, args.truth_spp)
    };
    let mut json = format!(
        "{{\n  \"scene\": \"{}\",\n  \"label\": \"{}\",\n  \"protocol\": \"{}\",\n  \"time_secs\": {},\n  \"truth_spp\": {},\n  \"timestamp\": {stamp},\n  \"default_config\": \"{}\",\n  \"truth_render\": \"{truth_render_name}\",\n  \"rows\": [",
        args.scene,
        args.label.replace('"', ""),
        match (args.accum, args.no_dlss, args.orbit.is_some()) {
            (true, ..) => "exam",
            (false, true, false) => "per-frame",
            (false, true, true) => "per-frame+motion",
            (false, false, false) => "per-frame+dlss",
            (false, false, true) => "per-frame+dlss+motion",
        },
        args.time,
        args.truth_spp,
        default_config.replace('\\', "\\\\").replace('"', "\\\""),
    );
    for (i, row) in rows.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let opt = |v: Option<f64>| v.map_or("null".to_string(), |v| v.to_string());
        json.push_str(&format!(
            "{sep}\n    {{\"recipe\": \"{}\", \"flip\": {}, \"flicker\": {}, \"frame_ms\": {}, \"spp\": {}, \"map\": {}, \"render\": {}, \"camera\": {}}}",
            row.recipe,
            opt(row.flip),
            opt(row.flicker),
            opt(row.frame_ms),
            row.spp.map_or("null".to_string(), |v| v.to_string()),
            row.map.as_ref().map_or("null".to_string(), |m| format!("\"{m}\"")),
            row.render.as_ref().map_or("null".to_string(), |m| format!("\"{m}\"")),
            row.camera.as_ref().map_or("null".to_string(), |c| {
                format!("\"{}\"", c.replace('\\', "\\\\").replace('"', "\\\""))
            }),
        ));
    }
    json.push_str("\n  ]\n}\n");
    let json_path = runs_dir.join(name);
    std::fs::write(&json_path, json).expect("write run json");
    println!("run json: {}", json_path.display());
}

struct FurnaceRun {
    /// `SolariLighting::default()` as the child printed it at startup.
    default_config: Option<String>,
    /// Path of the EXR the run dumped, if any.
    dump: Option<PathBuf>,
    /// Accumulated spp at dump time (parsed from the dump log line).
    dump_spp: Option<u32>,
    /// PNG screenshots the run captured, in order (post-DLSS-RR,
    /// post-tonemap — the per-frame protocol's burst).
    shots: Vec<PathBuf>,
    /// The effective `SolariCamera` the run printed as RON — replayable via
    /// `--camera`.
    camera_ron: Option<String>,
    /// The name furnace derived from the config — row/file identity.
    camera_name: Option<String>,
    /// Furnace's scene fingerprint (source hash + scene) — stale-truth guard.
    scene_fp: Option<String>,
    /// Per-second `frame_time` averages (ms) from `--fps` diagnostics.
    frame_ms: Vec<f64>,
    /// The run was killed early: the window settled at a size other than the
    /// truth's (tiling WMs deal their own) — relaunch instead of wasting the
    /// full budget on a dump FLIP can't compare.
    wrong_size: bool,
}

/// Launch `solari_furnace` with the scene + `extra` args, stream its output,
/// and return the parsed results. `kill_on_dump` ends the run the moment the
/// dump lands (truth bakes have no natural timeout).
fn run_furnace(
    args: &Args,
    extra: &[String],
    timeout: f32,
    kill_on_dump: bool,
    expect_dims: Option<&str>,
) -> FurnaceRun {
    let mut child = Command::new("cargo")
        .args(["run", "--release", "-p", "solari_furnace", "--"])
        .args(["--scene", &args.scene, "-t", &timeout.to_string()])
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cargo run solari_furnace");

    // Bevy logs to stdout, cargo/panics to stderr; watch both.
    let (tx, rx) = mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let tx2 = tx.clone();
    let readers = [
        std::thread::spawn(move || forward_lines(stdout, &tx)),
        std::thread::spawn(move || forward_lines(stderr, &tx2)),
    ];

    let mut run = FurnaceRun {
        default_config: None,
        dump: None,
        dump_spp: None,
        shots: Vec::new(),
        camera_ron: None,
        camera_name: None,
        scene_fp: None,
        frame_ms: Vec::new(),
        wrong_size: false,
    };
    // Latest `solari window: WxH` announcement; enforced at each frame_time
    // tick (by then the WM has settled the size), not on first sight — a
    // tiler may deal a transient size before the final one.
    let mut window_dims = String::new();
    for line in rx {
        let line = strip_ansi(&line);
        if let Some(config) = line.split("solari_default: ").nth(1) {
            run.default_config = Some(config.trim().to_string());
        }
        if let Some(ron) = line.split("solari_camera: ").nth(1) {
            run.camera_ron = Some(ron.trim().to_string());
        }
        if let Some(name) = line.split("solari_camera_name: ").nth(1) {
            run.camera_name = Some(name.trim().to_string());
        }
        if let Some(fp) = line.split("solari_scene_fp: ").nth(1) {
            run.scene_fp = Some(fp.trim().to_string());
        }
        if let Some(dims) = line.split("solari window: ").nth(1) {
            window_dims = dims.trim().to_string();
        }
        // `solari dump: wrote target/tmp/solari-WxH-Nspp-E.exr (N spp)`
        if let Some(rest) = line.split("solari dump: wrote ").nth(1) {
            let mut parts = rest.split(' ');
            if let Some(path) = parts.next() {
                run.dump = Some(PathBuf::from(path));
                run.dump_spp = rest
                    .split('(')
                    .nth(1)
                    .and_then(|s| s.split(' ').next())
                    .and_then(|s| s.parse().ok());
            }
        }
        // `solari shot: wrote target/tmp/solari-shot-MS.png`
        if let Some(path) = line.split("solari shot: wrote ").nth(1) {
            run.shots.push(PathBuf::from(path.trim()));
        }
        // Truth bakes run until their captures land: the EXR dump, plus the
        // display-referred shot twin when `--shot-on-dump` is among the args.
        if kill_on_dump
            && run.dump.is_some()
            && (!run.shots.is_empty() || !extra.iter().any(|a| a == "--shot-on-dump"))
        {
            let _ = child.kill();
        }
        // `frame_time   :   8.123456ms (avg 8.234567ms)`
        if line.contains("frame_time")
            && let Some(avg) = line.split("(avg ").nth(1)
            && let Some(ms) = avg.split("ms").next()
            && let Ok(value) = ms.trim().parse::<f64>()
        {
            run.frame_ms.push(value);
            if let Some(expect) = expect_dims
                && !window_dims.is_empty()
                && window_dims != expect
                && !run.wrong_size
            {
                eprintln!("  window is {window_dims}, truth is {expect} — aborting for retry");
                run.wrong_size = true;
                let _ = child.kill();
            }
        }
        if line.contains("panicked") || line.contains("DEVICE_LOST") {
            eprintln!("  furnace: {line}");
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    let _ = child.wait();
    run
}

/// Forward each line of `source` into the channel until EOF.
fn forward_lines(source: impl Read, tx: &mpsc::Sender<String>) {
    for line in BufReader::new(source).lines().map_while(Result::ok) {
        if tx.send(line).is_err() {
            return;
        }
    }
}

/// Drop ANSI color escape sequences (bevy's log output keeps them when piped).
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Median of the steady-state half (skips warmup seconds).
fn median(samples: &[f64]) -> Option<f64> {
    let tail = &samples[samples.len() / 2..];
    if tail.is_empty() {
        return None;
    }
    let mut sorted = tail.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(sorted[sorted.len() / 2])
}

/// The `{w}x{h}` token of a dump filename (`solari-WxH-Nspp-E.exr`).
fn dump_dims(path: &Path) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_prefix("solari-")?
        .split('-')
        .next()
        .map(str::to_string)
}

/// A PNG's "WxH", from its header (same shape as the truth.dims sidecar).
fn png_dims(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let info = png::Decoder::new(std::io::BufReader::new(file)).read_info().ok()?;
    let (w, h) = info.info().size();
    Some(format!("{w}x{h}"))
}

/// FLIP two images, mean only (no maps) — the flicker pair scorer.
fn flip_mean_only(flip: &Path, reference: &Path, test: &Path) -> Option<f64> {
    let output = Command::new(flip)
        .arg("-r")
        .arg(reference)
        .arg("-t")
        .arg(test)
        .args(["-v", "1", "--no-error-map", "--no-exposure-map"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("Mean: "))
        .and_then(|v| v.trim().parse().ok())
}

/// Run HDR-FLIP: parse the mean and write the error-map heatmap PNG
/// (`<recipe>.png`) into the scene dir for the report's flip column.
fn run_flip(
    flip: &Path,
    reference: &Path,
    test: &Path,
    dir: &Path,
    base: &str,
) -> Option<f64> {
    let mut cmd = Command::new(flip);
    cmd.arg("-r").arg(reference).arg("-t").arg(test);
    cmd.args(["-v", "1", "-b", base, "--no-exposure-map", "-d"]).arg(dir);
    let output = cmd.output().expect("run flip");
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        eprintln!("  flip failed: {}", String::from_utf8_lossy(&output.stderr));
        return None;
    }
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Mean: "))
        .and_then(|v| v.trim().parse().ok())
}

/// Read an RGB EXR into (width, height, rgb f32 pixels, top-down).
fn read_exr(path: &Path) -> (usize, usize, Vec<f32>) {
    use exr::prelude::*;
    let image = read_first_rgba_layer_from_file(
        path,
        |size: Vec2<usize>, _| (size.x(), size.y(), vec![0f32; size.x() * size.y() * 3]),
        |buf: &mut (usize, usize, Vec<f32>), pos: Vec2<usize>, (r, g, b, _): (f32, f32, f32, f32)| {
            let i = (pos.y() * buf.0 + pos.x()) * 3;
            buf.2[i] = r;
            buf.2[i + 1] = g;
            buf.2[i + 2] = b;
        },
    )
    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    image.layer_data.channel_data.pixels
}

/// Photographic auto-exposure: key 0.18 over the log-average luminance.
fn auto_exposure(rgb: &[f32]) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for px in rgb.chunks_exact(3) {
        let lum = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
        if lum.is_finite() {
            sum += f64::from(lum.max(1e-6)).ln();
            count += 1;
        }
    }
    if count == 0 {
        return 1.0;
    }
    0.18 / (sum / count as f64).exp() as f32
}

/// Tonemap (exposure -> ACES fit -> sRGB) and write an 8-bit RGB PNG.
fn write_render_png(path: &Path, image: &(usize, usize, Vec<f32>), exposure: f32) {
    let (w, h, rgb) = image;
    let aces = |x: f32| {
        ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
    };
    let srgb = |c: f32| {
        if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    let buf: Vec<u8> = rgb
        .iter()
        .map(|&c| {
            let c = if c.is_finite() { c } else { 0.0 };
            (srgb(aces(c * exposure)) * 255.0 + 0.5) as u8
        })
        .collect();
    let file = std::fs::File::create(path)
        .unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), *w as u32, *h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&buf).expect("png data");
}
