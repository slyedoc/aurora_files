//! Repo task runner for the solari_files workspace: `cargo xtask <task>`.
//!
//! Every task runs from the workspace root regardless of where it's invoked,
//! and pass-through args flow to the underlying tool unchanged:
//!
//!   cargo xtask furnace --scene room --camera '(mode: Reference((di: Restir(()))))'
//!   cargo xtask grade --scene lamps --time 20 --label baseline
//!   cargo xtask view assets/san_miguel/SanMiguel.bsn --camera '(mode: Realtime(()))' --bench 30
//!   cargo xtask soak 10
//!
//! The grader rebuilds the HTML report itself after every run; for a one-off
//! regeneration with different flags: `python3 scripts/grade_report.py --embed-maps`.

use std::path::Path;
use std::process::{Command, exit};

use clap::Parser;

#[derive(Parser)]
#[command(about = "solari_files task runner", bin_name = "cargo xtask")]
enum Task {
    /// Run the furnace exam harness (release; args pass through).
    Furnace(Passthrough),
    /// Run the recipe grader (release; args pass through) — it rebuilds the
    /// HTML report itself and prints the link.
    Grade(Passthrough),
    /// Run the .bsn viewer (release; args pass through).
    View(Passthrough),
    /// Bake the grader's converged truth caches (all furnace scenes unless
    /// `--scene` is passed; extra args forward to the grader, e.g.
    /// `--truth-spp 16384` or `--rebake-truth`).
    Truth(Passthrough),
    /// Open the HTML report in the browser (always regenerated first).
    Report,
    /// List recorded grader runs; `--rm <MATCH>` deletes matching ones.
    Runs(RunsArgs),
    /// Cold-start soak: N fresh furnace launches, tripwire-scanned
    /// (scripts/solari_soak.sh).
    Soak(Passthrough),
}

#[derive(clap::Args)]
struct RunsArgs {
    /// Remove runs whose scene, label, or filename contains MATCH; bare
    /// `--rm` removes every run. Truth caches are kept — only run records go.
    #[arg(long, value_name = "MATCH", num_args = 0..=1, default_missing_value = "all")]
    rm: Option<String>,
}

#[derive(clap::Args)]
struct Passthrough {
    /// Arguments forwarded to the underlying tool unchanged.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() {
    // crates/xtask/ -> the workspace root; every task runs from there (dumps,
    // truth caches, asset paths, and reports all key off it).
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");

    let status = match Task::parse() {
        Task::Furnace(p) => tool(root, "solari_furnace", &p.args),
        Task::View(p) => tool(root, "bsn", &p.args),
        Task::Grade(p) => tool(root, "solari_grader", &p.args),
        Task::Truth(p) => truth(root, &p.args),
        Task::Report => report(root),
        Task::Runs(a) => runs(root, a.rm.as_deref()),
        Task::Soak(p) => {
            let mut args = vec!["scripts/solari_soak.sh".to_string()];
            args.extend(p.args);
            run(root, "bash", &args)
        }
    };
    exit(status);
}

/// Bake truth caches: one grader `--truth-only` run per scene. A `--scene`
/// in the forwarded args bakes just that one; otherwise every furnace scene.
fn truth(root: &Path, args: &[String]) -> i32 {
    let scenes: &[&str] = if args.iter().any(|a| a == "--scene") {
        &[""]
    } else {
        &["furnace", "room", "yard", "bistro"]
    };
    for scene in scenes {
        let mut forwarded = vec!["--truth-only".to_string()];
        if !scene.is_empty() {
            println!("== truth: {scene}");
            forwarded.extend(["--scene".into(), (*scene).into()]);
        }
        forwarded.extend(args.iter().cloned());
        let code = tool(root, "solari_grader", &forwarded);
        if code != 0 {
            return code;
        }
    }
    0
}

/// Open the report in the browser, regenerating it first (sub-second, and it
/// keeps a template/script change from opening a stale page).
fn report(root: &Path) -> i32 {
    let code = run(root, "python3", &["scripts/grade_report.py".into()]);
    if code != 0 {
        return code;
    }
    let html = root.join("target/solari_grader/report.html");
    run(root, "xdg-open", &[html.display().to_string()])
}

/// List every recorded grader run (scene / label / when / path); with a
/// match string, delete the matching ones and refresh the report.
fn runs(root: &Path, rm: Option<&str>) -> i32 {
    let mut files: Vec<_> = std::fs::read_dir(root.join("target/solari_grader"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|scene| {
            let runs = scene.path().join("runs");
            runs.is_dir().then(|| std::fs::read_dir(runs))
        })
        .flatten()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    if files.is_empty() {
        println!("no recorded runs under target/solari_grader/*/runs/");
        return 0;
    }

    let mut removed = 0;
    println!("{:<10} {:<20} {:<17} path", "scene", "label", "when (utc)");
    for path in &files {
        let scene = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map_or_else(|| "?".into(), |s| s.to_string_lossy().into_owned());
        let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let (ts, label) = match stem.split_once('-') {
            Some((ts, label)) => (ts, label),
            None => (stem.as_str(), ""),
        };
        let when = ts.parse::<u64>().map_or_else(|_| "?".into(), utc);
        let matched = rm.is_some_and(|m| {
            m == "all" || scene.contains(m) || label.contains(m) || stem.contains(m)
        });
        if matched {
            match std::fs::remove_file(path) {
                Ok(()) => {
                    removed += 1;
                    println!("{scene:<10} {label:<20} {when:<17} {} REMOVED", path.display());
                }
                Err(e) => eprintln!("failed to remove {}: {e}", path.display()),
            }
        } else {
            println!("{scene:<10} {label:<20} {when:<17} {}", path.display());
        }
    }

    if removed > 0 {
        println!("removed {removed} run(s); truth caches kept");
        // Refresh (or retire) the report so it reflects what's left.
        let html = root.join("target/solari_grader/report.html");
        if removed == files.len() {
            let _ = std::fs::remove_file(html);
        } else {
            run(root, "python3", &["scripts/grade_report.py".into()]);
        }
    }
    0
}

/// Unix seconds -> "YYYY-MM-DD HH:MM" (UTC; civil-from-days, no chrono dep).
fn utc(ts: u64) -> String {
    let (days, secs) = (ts / 86400, ts % 86400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

/// `cargo run --release -p <package> -- <args>` from the workspace root.
fn tool(root: &Path, package: &str, args: &[String]) -> i32 {
    let mut cargo_args: Vec<String> =
        ["run", "--release", "-p", package, "--"].map(String::from).into();
    cargo_args.extend(args.iter().cloned());
    run(root, "cargo", &cargo_args)
}

/// Run a command from the workspace root, inheriting stdio; returns the exit code.
fn run(root: &Path, program: &str, args: &[String]) -> i32 {
    let display = format!("{program} {}", args.join(" "));
    match Command::new(program).args(args).current_dir(root).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("xtask: failed to run `{display}`: {e}");
            1
        }
    }
}
