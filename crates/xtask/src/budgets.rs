//! The budget registry, `perf/budgets.toml` (contract; P1-P15).
//!
//! One `[[budget]]` table per budget. See the header of the file for the meaning of every field and for the
//! `registered` semantics that `bench-check` applies (plan Q5).

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

/// Which statistic of the samples a budget is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Statistic {
    Min,
    P50,
    P95,
    P99,
    Mean,
    Max,
}

impl Statistic {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::P99 => "p99",
            Self::Mean => "mean",
            Self::Max => "max",
        }
    }
}

/// Whether the threshold is a ceiling or a floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// The measured value must be at most the threshold (latency, memory, bytes).
    #[default]
    Upper,
    /// The measured value must be at least the threshold (frames per second, score, throughput).
    Lower,
}

/// One budget.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub id: String,
    pub description: String,
    /// `<group>/<function>`: a criterion benchmark, or the `bench` name inside a load-test result file.
    pub bench: String,
    pub statistic: Statistic,
    pub threshold: f64,
    pub unit: String,
    /// `small` or `target`: the fixture scale the measurement is made at.
    pub scale: String,
    #[serde(default)]
    pub direction: Direction,
    /// False until the owning prompt has a benchmark that produces the result.
    pub registered: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    budget: Vec<Budget>,
}

/// The parsed registry.
#[derive(Debug, Clone, PartialEq)]
pub struct Registry {
    pub budgets: Vec<Budget>,
}

/// Why a registry was rejected. Messages name the entry, never file contents.
#[derive(Debug)]
pub enum RegistryError {
    Read(String),
    Toml(String),
    DuplicateId(String),
    Invalid { id: String, reason: &'static str },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) => write!(f, "cannot read the budget registry: {e}"),
            Self::Toml(e) => write!(f, "the budget registry is not valid: {e}"),
            Self::DuplicateId(id) => write!(f, "duplicate budget id {id}"),
            Self::Invalid { id, reason } => write!(f, "budget {id}: {reason}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl Registry {
    /// Reads and validates the registry file.
    ///
    /// # Errors
    /// When the file cannot be read, is not valid TOML for the schema, or an entry breaks a rule.
    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| RegistryError::Read(format!("{}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Parses and validates registry text.
    ///
    /// # Errors
    /// When the text is not valid for the schema or an entry breaks a rule.
    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        let file: File = toml::from_str(text).map_err(|e| RegistryError::Toml(e.to_string()))?;
        let mut seen = BTreeSet::new();
        for b in &file.budget {
            validate(b)?;
            if !seen.insert(b.id.clone()) {
                return Err(RegistryError::DuplicateId(b.id.clone()));
            }
        }
        Ok(Self { budgets: file.budget })
    }
}

fn validate(b: &Budget) -> Result<(), RegistryError> {
    let bad = |reason: &'static str| RegistryError::Invalid {
        id: b.id.clone(),
        reason,
    };
    let id_ok =
        b.id.strip_prefix('P')
            .and_then(|rest| rest.split('.').next())
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
    if !id_ok {
        return Err(bad("id must be P<number> or P<number>.<name>"));
    }
    if b.description.trim().is_empty() || b.unit.trim().is_empty() {
        return Err(bad("description and unit must not be empty"));
    }
    if !(b.threshold.is_finite() && b.threshold > 0.0) {
        return Err(bad("threshold must be a positive number"));
    }
    if !matches!(b.scale.as_str(), "small" | "target") {
        return Err(bad("scale must be small or target"));
    }
    // `bench` is joined onto a results directory, so it must stay inside it.
    let parts: Vec<&str> = b.bench.split('/').collect();
    if parts.len() < 2
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\\'))
    {
        return Err(bad(
            "bench must be <group>/<function> with no empty, . or .. component",
        ));
    }
    Ok(())
}
