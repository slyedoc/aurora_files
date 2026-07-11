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
//! The truth is a `--recipe reference` run accumulated to `--truth-spp`,
//! cached under `target/solari_grader/<scene>/` so it bakes once.
//!
//! Dumps happen pre-DLSS-RR (the RT output buffer), so the default recipe is
//! graded on its raw estimator output; RR's temporal polish is a separate
//! question (flicker grading is a follow-up).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use clap::Parser;

/// Every recipe the CLI knows (`SolariRecipe` names).
const ALL_RECIPES: &[&str] = &[
    "reference",
    "bsdf",
    "restir-di",
    "restir-di-spatial",
    "restir-gi",
    "restir-gi-spatial",
    "nrc",
    "default",
];

#[derive(Parser)]
#[command(about = "Grade solari recipes on accuracy (HDR-FLIP vs converged truth) and speed")]
struct Args {
    /// furnace scene to grade on (furnace | room | lamps | yard | cell)
    #[arg(long, default_value = "room")]
    scene: String,

    /// recipes to grade (comma-separated), or "all"
    #[arg(long, default_value = "all", value_delimiter = ',')]
    recipes: Vec<String>,

    /// equal-time budget: seconds each recipe accumulates before its dump
    #[arg(long, default_value_t = 20.0)]
    time: f32,

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

    /// also write FLIP error-map heatmaps next to the report
    #[arg(long)]
    maps: bool,

    /// path to the FLIP CLI binary (or set FLIP_BIN)
    #[arg(long)]
    flip: Option<PathBuf>,


    /// label recorded in the run's JSON (e.g. "baseline", "nrc-anchor-fix") —
    /// the report's compare view diffs labeled runs
    #[arg(long, default_value = "")]
    label: String,

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

    let recipes: Vec<&str> = if args.recipes.iter().any(|r| r == "all") {
        ALL_RECIPES.to_vec()
    } else {
        args.recipes.iter().map(String::as_str).collect()
    };

    // Truth: reference accumulation to --truth-spp, cached per scene.
    let truth_exr = out_dir.join(format!("truth-{}spp.exr", args.truth_spp));
    // The dims sidecar doubles as the cache-format marker: caches from the old
    // PFM-converting grader lack it (and their EXRs crash FLIP) — rebake.
    let truth_valid = truth_exr.is_file() && out_dir.join("truth.dims").is_file();
    if args.rebake_truth || !truth_valid {
        println!(
            "baking truth: {} @ {} spp (cached at {})",
            args.scene,
            args.truth_spp,
            truth_exr.display()
        );
        let run = run_furnace(
            &args,
            &[
                "--accum".into(),
                "--spp".into(),
                "32".into(),
                "--dump-at-spp".into(),
                args.truth_spp.to_string(),
            ],
            // Truth bakes until the spp threshold fires the dump; generous cap.
            3600.0,
            true,
        );
        let dump = run.dump.expect("truth run produced no dump — see its output above");
        std::fs::copy(&dump, &truth_exr).expect("cache truth exr");
        std::fs::write(out_dir.join("truth.dims"), dump_dims(&dump).unwrap_or_default())
            .expect("record truth dims");
    } else {
        println!("truth: cached {}", truth_exr.display());
    }
    // One exposure for the whole scene, derived from the truth — every render
    // PNG (truth + candidates) is exposed identically, so they compare fairly.
    let truth_pixels = read_exr(&truth_exr);
    let exposure = auto_exposure(&truth_pixels.2);
    let truth_render = out_dir.join(format!("truth-{}spp.render.png", args.truth_spp));
    if args.rebake_truth || !truth_render.is_file() {
        write_render_png(&truth_render, &truth_pixels, exposure);
    }
    if args.truth_only {
        return;
    }
    let truth_dims = std::fs::read_to_string(out_dir.join("truth.dims")).unwrap_or_default();

    // Candidates: equal-time accumulation per recipe, then FLIP vs truth.
    let mut rows = Vec::new();
    let mut default_config = String::new();
    for recipe in &recipes {
        println!("grading: --recipe {recipe} ({}s)", args.time);
        let run = run_furnace(
            &args,
            // The accuracy protocol is pinned here, not inherited from CLI
            // defaults: equal-time ACCUMULATION at 16 spp/frame.
            &[
                "--recipe".into(),
                (*recipe).into(),
                "--accum".into(),
                "--spp".into(),
                "16".into(),
                "--dump-at-secs".into(),
                args.time.to_string(),
                "--fps".into(),
            ],
            args.time + 4.0,
            false,
        );
        if default_config.is_empty()
            && let Some(config) = &run.default_config
        {
            default_config = config.clone();
        }
        let Some(dump) = run.dump else {
            eprintln!("  {recipe}: no dump produced — skipped");
            continue;
        };
        // FLIP needs identical resolutions; the window size decides the dump's.
        // A mismatch means the truth was baked at a different window size.
        let dims = dump_dims(&dump).unwrap_or_default();
        if !truth_dims.is_empty() && dims != truth_dims {
            eprintln!(
                "  {recipe}: dump is {dims} but the truth is {truth_dims} — keep the window \
                 size consistent (or --rebake-truth); skipped"
            );
            continue;
        }
        let exr = out_dir.join(format!("{recipe}.exr"));
        std::fs::copy(&dump, &exr).expect("copy candidate exr");
        let render = format!("{recipe}.render.png");
        write_render_png(&out_dir.join(&render), &read_exr(&exr), exposure);
        let flip_mean = run_flip(&flip, &truth_exr, &exr, args.maps.then_some((&out_dir, *recipe)));
        rows.push(Row {
            recipe: (*recipe).to_string(),
            flip: flip_mean,
            frame_ms: median(&run.frame_ms),
            spp: run.dump_spp,
            map: args.maps.then(|| format!("{recipe}.png")),
            render: Some(render),
        });
    }

    report(&args, &out_dir, &mut rows, &default_config);

    // Rebuild the HTML report so it always reflects the latest run, and print
    // an openable link (the script scans every scene's recorded runs).
    let script = Path::new("scripts/grade_report.py");
    if script.is_file() {
        let ok = Command::new("python3")
            .arg(script)
            .status()
            .is_ok_and(|s| s.success());
        if ok && let Ok(abs) = std::fs::canonicalize("target/solari_grader/report.html") {
            println!("open: file://{}", abs.display());
        }
    } else {
        eprintln!("html report skipped: {} not found (run from the workspace root)", script.display());
    }
}

struct Row {
    recipe: String,
    flip: Option<f64>,
    frame_ms: Option<f64>,
    spp: Option<u32>,
    /// FLIP error-map filename in the scene dir (with `--maps`).
    map: Option<String>,
    /// Tonemapped render filename in the scene dir.
    render: Option<String>,
}

/// Sort by FLIP (best first), print the table, and write the CSV.
fn report(args: &Args, out_dir: &Path, rows: &mut [Row], default_config: &str) {
    rows.sort_by(|a, b| {
        a.flip
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.flip.unwrap_or(f64::INFINITY))
    });
    println!();
    println!(
        "scene {} | {}s equal-time | truth {} spp | FLIP·ms = error × cost (lower is better on every column)",
        args.scene, args.time, args.truth_spp
    );
    println!(
        "{:<20} {:>8} {:>9} {:>9} {:>7}",
        "recipe", "FLIP", "avg ms", "FLIP·ms", "spp"
    );
    let mut csv = String::from("recipe,flip_mean,avg_frame_ms,flip_x_ms,spp\n");
    for row in rows.iter() {
        let flip = row.flip.map_or("-".into(), |v| format!("{v:.4}"));
        let ms = row.frame_ms.map_or("-".into(), |v| format!("{v:.2}"));
        let product = row
            .flip
            .zip(row.frame_ms)
            .map_or("-".into(), |(f, m)| format!("{:.3}", f * m));
        let spp = row.spp.map_or("-".into(), |v| v.to_string());
        println!("{:<20} {:>8} {:>9} {:>9} {:>7}", row.recipe, flip, ms, product, spp);
        csv.push_str(&format!(
            "{},{},{},{},{}\n",
            row.recipe,
            row.flip.map_or(String::new(), |v| v.to_string()),
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
    let mut json = format!(
        "{{\n  \"scene\": \"{}\",\n  \"label\": \"{}\",\n  \"time_secs\": {},\n  \"truth_spp\": {},\n  \"timestamp\": {stamp},\n  \"default_config\": \"{}\",\n  \"truth_render\": \"truth-{}spp.render.png\",\n  \"rows\": [",
        args.scene,
        args.label.replace('"', ""),
        args.time,
        args.truth_spp,
        default_config.replace('\\', "\\\\").replace('"', "\\\""),
        args.truth_spp,
    );
    for (i, row) in rows.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let opt = |v: Option<f64>| v.map_or("null".to_string(), |v| v.to_string());
        json.push_str(&format!(
            "{sep}\n    {{\"recipe\": \"{}\", \"flip\": {}, \"frame_ms\": {}, \"spp\": {}, \"map\": {}, \"render\": {}}}",
            row.recipe,
            opt(row.flip),
            opt(row.frame_ms),
            row.spp.map_or("null".to_string(), |v| v.to_string()),
            row.map.as_ref().map_or("null".to_string(), |m| format!("\"{m}\"")),
            row.render.as_ref().map_or("null".to_string(), |m| format!("\"{m}\"")),
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
    /// Per-second `frame_time` averages (ms) from `--fps` diagnostics.
    frame_ms: Vec<f64>,
}

/// Launch `solari_furnace` with the scene + `extra` args, stream its output,
/// and return the parsed results. `kill_on_dump` ends the run the moment the
/// dump lands (truth bakes have no natural timeout).
fn run_furnace(args: &Args, extra: &[String], timeout: f32, kill_on_dump: bool) -> FurnaceRun {
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

    let mut run = FurnaceRun { default_config: None, dump: None, dump_spp: None, frame_ms: Vec::new() };
    for line in rx {
        let line = strip_ansi(&line);
        if let Some(config) = line.split("solari_default: ").nth(1) {
            run.default_config = Some(config.trim().to_string());
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
            if kill_on_dump {
                let _ = child.kill();
            }
        }
        // `frame_time   :   8.123456ms (avg 8.234567ms)`
        if line.contains("frame_time")
            && let Some(avg) = line.split("(avg ").nth(1)
            && let Some(ms) = avg.split("ms").next()
            && let Ok(value) = ms.trim().parse::<f64>()
        {
            run.frame_ms.push(value);
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

/// Run HDR-FLIP and parse the mean; `map` = (dir, basename) also writes the
/// error-map heatmap PNG there.
fn run_flip(
    flip: &Path,
    reference: &Path,
    test: &Path,
    map: Option<(&PathBuf, &str)>,
) -> Option<f64> {
    let mut cmd = Command::new(flip);
    cmd.arg("-r").arg(reference).arg("-t").arg(test);
    match map {
        Some((dir, base)) => {
            cmd.args(["-v", "1", "-b", base, "--no-exposure-map", "-d"]).arg(dir);
        }
        None => {
            cmd.args(["-v", "1", "--no-error-map", "--no-exposure-map"]);
        }
    };
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
