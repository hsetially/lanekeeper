//! `cargo xtask bench-check`: compares measured results with the budgets of `perf/budgets.toml`.
//!
//! Where results come from:
//! - criterion benchmarks: `<criterion_dir>/<bench>/new/sample.json`, which holds one entry per sample (`iters[i]`
//!   iterations took `times[i]` nanoseconds). The per-iteration time of sample `i` is `times[i] / iters[i]`, and the
//!   budget's statistic is computed over those values, so p95 is a real percentile and not criterion's mean estimate.
//!   Each file is read on its own and dropped, so the cost does not depend on the size of the criterion tree;
//! - load tests, bundle size, Lighthouse and memory checks: a JSON file in `<results_dir>` with the same `bench` name,
//!   `{"bench": "...", "unit": "...", "value": 1.0}` or `{"bench": "...", "unit": "...", "values": {"p95": 1.0}}`.
//!
//! Semantics (plan Q5):
//! - an entry with `registered = true` fails when it is over budget (or under its floor), has no result, or has a
//!   result that cannot be trusted (unreadable, wrong unit, empty);
//! - an entry with `registered = false` is listed as UNMET with whatever was measured and only fails with `--strict`,
//!   which the load-test gate (prompt 16) and releases use.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::budgets::{Budget, Direction, Registry, RegistryError, Statistic};

/// A result file larger than this is not read (a sample file has a few hundred numbers).
const MAX_RESULT_BYTES: u64 = 16 * 1024 * 1024;
/// A results directory with more entries than this is not scanned further.
const MAX_RESULT_FILES: usize = 10_000;

/// Where to read from.
#[derive(Debug, Clone)]
pub struct Options {
    pub budgets: PathBuf,
    pub criterion_dir: PathBuf,
    pub results_dir: PathBuf,
    pub strict: bool,
}

/// The decision for one budget.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Pass {
        measured: f64,
    },
    /// Registered, upper bound, and the measurement is above the threshold.
    OverBudget {
        measured: f64,
    },
    /// Registered, lower bound, and the measurement is below the threshold.
    UnderFloor {
        measured: f64,
    },
    /// Registered, and there is no result at all.
    MissingResult,
    /// Registered, and the result cannot be trusted.
    BadResult {
        reason: String,
    },
    /// Not registered: reported, and a failure only with `--strict`.
    Unmet {
        reason: String,
    },
}

impl Verdict {
    fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::OverBudget { .. } | Self::UnderFloor { .. } | Self::MissingResult | Self::BadResult { .. }
        )
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub bench: String,
    pub statistic: Statistic,
    pub threshold: f64,
    pub unit: String,
    pub direction: Direction,
    pub registered: bool,
    pub measured: Option<f64>,
    pub verdict: Verdict,
}

/// The whole result of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub outcomes: Vec<Outcome>,
    pub strict: bool,
}

impl Report {
    /// True when the gate must exit non-zero.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.outcomes
            .iter()
            .any(|o| o.verdict.is_failure() || (self.strict && matches!(o.verdict, Verdict::Unmet { .. })))
    }

    /// A table, one line per budget, then a summary line.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let (mut pass, mut fail, mut unmet) = (0_usize, 0_usize, 0_usize);
        for o in &self.outcomes {
            let (tag, note) = match &o.verdict {
                Verdict::Pass { .. } => {
                    pass += 1;
                    ("PASS ", String::new())
                }
                Verdict::OverBudget { .. } => {
                    fail += 1;
                    ("FAIL ", "over budget".to_owned())
                }
                Verdict::UnderFloor { .. } => {
                    fail += 1;
                    ("FAIL ", "under the floor".to_owned())
                }
                Verdict::MissingResult => {
                    fail += 1;
                    ("FAIL ", "registered but no result".to_owned())
                }
                Verdict::BadResult { reason } => {
                    fail += 1;
                    ("FAIL ", format!("bad result: {reason}"))
                }
                Verdict::Unmet { reason } => {
                    unmet += 1;
                    (if self.strict { "FAIL " } else { "UNMET" }, reason.clone())
                }
            };
            let measured = o.measured.map_or_else(|| "-".to_owned(), fmt_num);
            let op = match o.direction {
                Direction::Upper => "<=",
                Direction::Lower => ">=",
            };
            let _ = writeln!(
                out,
                "{tag} {:<34} {:>12} {} {op} {} {}  {note}",
                o.id,
                measured,
                o.statistic.as_str(),
                fmt_num(o.threshold),
                o.unit,
            );
        }
        let mode = if self.strict { "strict" } else { "default" };
        let _ = writeln!(
            out,
            "bench-check ({mode}): {pass} pass, {fail} fail, {unmet} unmet of {} budgets",
            self.outcomes.len()
        );
        out
    }
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.3}")
    }
}

/// Why a run could not even be evaluated (as opposed to a budget failing).
#[derive(Debug)]
pub enum CheckError {
    Registry(RegistryError),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CheckError {}

impl From<RegistryError> for CheckError {
    fn from(e: RegistryError) -> Self {
        Self::Registry(e)
    }
}

/// Reads the registry and every result it names, and decides each budget.
///
/// # Errors
/// When the registry cannot be read or is invalid. A missing or broken result is a verdict, not an error.
pub fn run(opts: &Options) -> Result<Report, CheckError> {
    let registry = Registry::load(&opts.budgets)?;
    let results = ResultFiles::scan(&opts.results_dir);
    let outcomes = registry
        .budgets
        .iter()
        .map(|b| {
            let evidence = measure(b, &opts.criterion_dir, &results);
            judge(b, evidence)
        })
        .collect();
    Ok(Report {
        outcomes,
        strict: opts.strict,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Evidence

/// `Ok(None)`: there is no result. `Err`: there is one, and it cannot be used.
type Evidence = Result<Option<f64>, String>;

fn measure(b: &Budget, criterion_dir: &Path, results: &ResultFiles) -> Evidence {
    let sample = criterion_dir.join(&b.bench).join("new").join("sample.json");
    if sample.is_file() {
        let per_iter_ns = read_criterion_sample(&sample)?;
        let scale = ns_per_unit(&b.unit)
            .ok_or_else(|| format!("a criterion result is in time units, not {:?}", b.unit))?;
        let in_unit: Vec<f64> = per_iter_ns.iter().map(|ns| ns / scale).collect();
        return Ok(Some(statistic(&in_unit, b.statistic)));
    }
    results.value_for(b)
}

fn ns_per_unit(unit: &str) -> Option<f64> {
    match unit {
        "ns" => Some(1.0),
        "us" => Some(1_000.0),
        "ms" => Some(1_000_000.0),
        "s" => Some(1_000_000_000.0),
        _ => None,
    }
}

#[derive(Deserialize)]
struct CriterionSample {
    iters: Vec<f64>,
    times: Vec<f64>,
}

/// Per-iteration times in nanoseconds, from a criterion `sample.json`.
///
/// # Errors
/// When the file cannot be read, is not a sample file, or holds no usable samples.
pub fn read_criterion_sample(path: &Path) -> Result<Vec<f64>, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let reader = BufReader::new(file.take(MAX_RESULT_BYTES));
    let sample: CriterionSample = serde_json::from_reader(reader)
        .map_err(|e| format!("{} is not a criterion sample file: {e}", path.display()))?;
    if sample.iters.is_empty() || sample.iters.len() != sample.times.len() {
        return Err(format!(
            "{}: needs the same, non-zero number of iters and times",
            path.display()
        ));
    }
    sample
        .iters
        .iter()
        .zip(&sample.times)
        .map(|(&iters, &time)| {
            if iters.is_finite() && iters > 0.0 && time.is_finite() && time >= 0.0 {
                Ok(time / iters)
            } else {
                Err(format!(
                    "{}: a sample has no iterations or a negative or non-finite time",
                    path.display()
                ))
            }
        })
        .collect()
}

/// Nearest-rank percentile of an ascending slice: the smallest value with at least `p` of the samples at or below it.
/// An empty slice gives NaN, which no threshold accepts.
#[must_use]
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    #[allow(clippy::cast_precision_loss)]
    let n = sorted.len() as f64;
    // The small epsilon keeps 0.95 * 100 from rounding up to 96.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rank = ((p * n) - 1e-9).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn statistic(values: &[f64], stat: Statistic) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    match stat {
        Statistic::Min => percentile(&sorted, 0.0),
        Statistic::P50 => percentile(&sorted, 0.50),
        Statistic::P95 => percentile(&sorted, 0.95),
        Statistic::P99 => percentile(&sorted, 0.99),
        Statistic::Max => percentile(&sorted, 1.0),
        Statistic::Mean => {
            #[allow(clippy::cast_precision_loss)]
            let n = sorted.len() as f64;
            sorted.iter().sum::<f64>() / n
        }
    }
}

/// The JSON files of `target/perf-results/`, keyed by the bench name they declare.
struct ResultFiles {
    parsed: Vec<(PathBuf, ResultFile)>,
}

#[derive(Deserialize)]
struct ResultFile {
    bench: String,
    unit: String,
    #[serde(default)]
    statistic: Option<String>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    values: std::collections::BTreeMap<String, f64>,
}

impl ResultFiles {
    fn scan(dir: &Path) -> Self {
        let mut parsed = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self { parsed };
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json") && p.is_file())
            .take(MAX_RESULT_FILES)
            .collect();
        paths.sort();
        for path in paths {
            let Ok(file) = File::open(&path) else { continue };
            // A file that does not parse is skipped here; the budget that needed it reports "no result".
            if let Ok(rf) =
                serde_json::from_reader::<_, ResultFile>(BufReader::new(file.take(MAX_RESULT_BYTES)))
            {
                parsed.push((path, rf));
            }
        }
        Self { parsed }
    }

    fn value_for(&self, b: &Budget) -> Evidence {
        let mut matching = self.parsed.iter().filter(|(_, rf)| rf.bench == b.bench);
        let Some((path, rf)) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Err(format!("more than one result file names {}", b.bench));
        }
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if rf.unit != b.unit {
            return Err(format!(
                "{name}: unit is {:?}, the budget is in {:?}",
                rf.unit, b.unit
            ));
        }
        let want = b.statistic.as_str();
        let value = if let Some(v) = rf.values.get(want) {
            *v
        } else if let Some(v) = rf.value {
            if rf.statistic.as_deref().is_some_and(|s| s != want) {
                return Err(format!(
                    "{name}: holds {:?}, the budget needs {want}",
                    rf.statistic
                ));
            }
            v
        } else {
            return Err(format!("{name}: has neither `value` nor `values.{want}`"));
        };
        if value.is_finite() {
            Ok(Some(value))
        } else {
            Err(format!("{name}: value is not a finite number"))
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Decision

fn within(b: &Budget, v: f64) -> bool {
    match b.direction {
        Direction::Upper => v <= b.threshold,
        Direction::Lower => v >= b.threshold,
    }
}

fn judge(b: &Budget, evidence: Evidence) -> Outcome {
    let (measured, verdict) = match (b.registered, evidence) {
        (true, Ok(Some(v))) => {
            let verdict = if within(b, v) {
                Verdict::Pass { measured: v }
            } else if b.direction == Direction::Upper {
                Verdict::OverBudget { measured: v }
            } else {
                Verdict::UnderFloor { measured: v }
            };
            (Some(v), verdict)
        }
        (true, Ok(None)) => (None, Verdict::MissingResult),
        (true, Err(reason)) => (None, Verdict::BadResult { reason }),
        (false, Ok(Some(v))) => {
            let state = if within(b, v) { "within" } else { "outside" };
            (
                Some(v),
                Verdict::Unmet {
                    reason: format!("not registered; measured {state} budget"),
                },
            )
        }
        (false, Ok(None)) => (
            None,
            Verdict::Unmet {
                reason: "not registered; no benchmark yet".to_owned(),
            },
        ),
        (false, Err(reason)) => (
            None,
            Verdict::Unmet {
                reason: format!("not registered; bad result: {reason}"),
            },
        ),
    };
    Outcome {
        id: b.id.clone(),
        bench: b.bench.clone(),
        statistic: b.statistic,
        threshold: b.threshold,
        unit: b.unit.clone(),
        direction: b.direction,
        registered: b.registered,
        measured,
        verdict,
    }
}
