//! Command-line shell for the xtask binary. Kept in the library so the argument rules are testable.

use std::path::{Path, PathBuf};

use crate::bench_check::{self, Options};
use crate::fixtures::{self, Config, DEFAULT_SEED, Scale};

/// Exit codes: 0 success, 1 the gate failed, 2 bad usage or an input that could not be read.
pub const EXIT_OK: u8 = 0;
pub const EXIT_GATE_FAILED: u8 = 1;
pub const EXIT_USAGE: u8 = 2;

pub const USAGE: &str = "usage:
  cargo xtask bench-check [--strict] [--budgets FILE] [--criterion-dir DIR] [--results-dir DIR]
  cargo xtask gen-fixtures --scale small|target [--seed N] [--out DIR]";

/// The workspace root, found from this crate's manifest directory (`crates/xtask`).
#[must_use]
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| workspace_root().join("target"), PathBuf::from)
}

/// Why the arguments were rejected.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Pulls the value of a `--flag VALUE` pair.
fn value<'a>(flag: &str, it: &mut impl Iterator<Item = &'a String>) -> Result<&'a String, UsageError> {
    it.next()
        .ok_or_else(|| UsageError(format!("{flag} needs a value")))
}

/// Parses the arguments after `bench-check`.
///
/// # Errors
/// On an unknown flag or a flag without its value.
pub fn parse_bench_check(args: &[String]) -> Result<Options, UsageError> {
    let target = target_dir();
    let mut opts = Options {
        budgets: workspace_root().join("perf/budgets.toml"),
        criterion_dir: target.join("criterion"),
        results_dir: target.join("perf-results"),
        strict: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--strict" => opts.strict = true,
            "--budgets" => opts.budgets = PathBuf::from(value(arg, &mut it)?),
            "--criterion-dir" => opts.criterion_dir = PathBuf::from(value(arg, &mut it)?),
            "--results-dir" => opts.results_dir = PathBuf::from(value(arg, &mut it)?),
            other => return Err(UsageError(format!("unknown argument {other}"))),
        }
    }
    Ok(opts)
}

/// Runs `bench-check` and returns the report text and the exit code.
#[must_use]
pub fn run_bench_check(args: &[String]) -> (String, u8) {
    let opts = match parse_bench_check(args) {
        Ok(o) => o,
        Err(e) => return (format!("{e}\n{USAGE}\n"), EXIT_USAGE),
    };
    match bench_check::run(&opts) {
        Ok(report) => {
            let code = if report.failed() {
                EXIT_GATE_FAILED
            } else {
                EXIT_OK
            };
            (report.render(), code)
        }
        Err(e) => (format!("bench-check: {e}\n"), EXIT_USAGE),
    }
}

/// Parses the arguments after `gen-fixtures`.
///
/// # Errors
/// On an unknown flag, a missing value or a missing `--scale`.
pub fn parse_gen_fixtures(args: &[String]) -> Result<Config, UsageError> {
    let mut scale = None;
    let mut seed = DEFAULT_SEED;
    let mut out = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--scale" => scale = Some(Scale::parse(value(arg, &mut it)?).map_err(UsageError)?),
            "--seed" => {
                let v = value(arg, &mut it)?;
                seed = v
                    .parse()
                    .map_err(|_| UsageError(format!("--seed needs a number, got {v:?}")))?;
            }
            "--out" => out = Some(PathBuf::from(value(arg, &mut it)?)),
            other => return Err(UsageError(format!("unknown argument {other}"))),
        }
    }
    let scale = scale.ok_or_else(|| UsageError("--scale small|target is required".to_owned()))?;
    let out = out.unwrap_or_else(|| target_dir().join("fixtures").join(scale.as_str()));
    Ok(Config { scale, seed, out })
}

/// Runs `gen-fixtures` and returns the summary text and the exit code.
#[must_use]
pub fn run_gen_fixtures(args: &[String]) -> (String, u8) {
    let cfg = match parse_gen_fixtures(args) {
        Ok(c) => c,
        Err(e) => return (format!("{e}\n{USAGE}\n"), EXIT_USAGE),
    };
    let started = std::time::Instant::now();
    match fixtures::generate(&cfg) {
        Ok(m) => (
            format!(
                "gen-fixtures: scale {} seed {}: {} swimlanes, {} tenant branches, {} base files, {} NFS files in {:.1?} -> {}\n",
                m.scale,
                m.seed,
                m.counts.swimlanes,
                m.counts.tenant_branches,
                m.counts.base_files,
                m.counts.nfs_files,
                started.elapsed(),
                cfg.out.display()
            ),
            EXIT_OK,
        ),
        Err(e) => (format!("gen-fixtures: {e}\n"), EXIT_USAGE),
    }
}
