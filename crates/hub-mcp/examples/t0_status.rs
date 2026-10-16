//! `just mcp-t0-status`: what is recorded of the Cursor 3.22.12 compatibility test (plan 07, T0; A4).
//!
//! The results table lives in D91 of `docs/decisions.md`. A cell says `NOT RUN` until a human has run that check on that
//! operating system. An agent cannot run Cursor 3.22.12 and never fills a cell, so this tool never counts `NOT RUN` as a pass:
//! it prints how much is recorded, per operating system, and exits 0. With `--strict` it exits 1 until every cell is recorded
//! and the recorded decisions follow the outcome rules of the prompt (a human runs that before go-live). A table it cannot read
//! is an error in both modes, because a status that cannot be determined must not look like one.
//!
//! The test file `tests/t0_status.rs` includes this file as a module and drives `run` on tables it builds.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

/// The text of a cell nobody has filled in.
pub const NOT_RUN: &str = "NOT RUN";
/// The operating systems of the table, in column order.
pub const OS: [&str; 2] = ["Windows", "macOS"];
/// The build T0 is about.
const CURSOR_BUILD: &str = "3.22.12";
/// The longest cell, in characters.
const MAX_CELL_CHARS: usize = 200;
/// The longest value shown in the rows listing.
const SHOWN_CHARS: usize = 40;

/// The three switches of `hub_mcp::config` that a result can decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Auth,
    Confirm,
    Content,
}

impl Setting {
    fn allowed(self) -> &'static [&'static str] {
        match self {
            Self::Auth => &["oauth", "personal_token"],
            Self::Confirm => &["elicitation", "draft"],
            Self::Content => &["summary", "data_block"],
        }
    }
}

/// What a row records, which decides which cell values are valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The Cursor build the checks ran on. Strict mode wants `3.22.12`.
    CursorBuild,
    Person,
    /// `YYYY-MM-DD`.
    Date,
    /// `PASS`, `FAIL` or `FAIL: reason`.
    Check,
    /// Free text: what was seen.
    Observation,
    /// The mode chosen for this operating system.
    Decision(Setting),
}

#[derive(Debug, Clone, Copy)]
pub struct RowSpec {
    pub id: &'static str,
    pub kind: Kind,
}

/// Every row D91 must have, in the order it prints them. `t0/README.md` says how to record each one.
pub const ROWS: &[RowSpec] = &[
    RowSpec {
        id: "META.cursor_build",
        kind: Kind::CursorBuild,
    },
    RowSpec {
        id: "META.recorded_by",
        kind: Kind::Person,
    },
    RowSpec {
        id: "META.date",
        kind: Kind::Date,
    },
    RowSpec {
        id: "C1.discovery",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C1.mirror_needed",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "C1.entra_accepts",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C1.tool_call",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C1.headers",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C2.prompt",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C2.accept",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C2.decline",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C2.cancel",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C2.no_hang",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C3.read_runs",
        kind: Kind::Check,
    },
    RowSpec {
        id: "C3.write_prompts",
        kind: Kind::Check,
    },
    RowSpec {
        id: "O.protocol_version",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "O.elicitation_capability",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "O.structured_content",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "O.error_vs_output_schema",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "O.session_404",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "O.origin_header",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "O.refresh",
        kind: Kind::Observation,
    },
    RowSpec {
        id: "D.auth",
        kind: Kind::Decision(Setting::Auth),
    },
    RowSpec {
        id: "D.confirm",
        kind: Kind::Decision(Setting::Confirm),
    },
    RowSpec {
        id: "D.content",
        kind: Kind::Decision(Setting::Content),
    },
];

/// Why D91 cannot be read. The messages name rows and columns, and repeat at most a row id, never a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusError {
    NoD91,
    MissingRow(String),
    DuplicateRow(String),
    UnknownRow(String),
    /// A row that is not `| id | what | Windows | macOS |`.
    BadRow(String),
    BadCell {
        row: String,
        os: &'static str,
        why: &'static str,
    },
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoD91 => f.write_str("there is no entry \"- **D91**\""),
            Self::MissingRow(id) => write!(f, "row {id} is missing"),
            Self::DuplicateRow(id) => write!(f, "row {id} appears twice"),
            Self::UnknownRow(id) => write!(f, "row {id} is not one this tool knows"),
            Self::BadRow(id) => write!(
                f,
                "row {id} does not have the four columns ID, what, Windows, macOS"
            ),
            Self::BadCell { row, os, why } => write!(f, "{os} cell of row {row}: {why}"),
        }
    }
}

impl std::error::Error for StatusError {}

/// The cells of D91, validated.
#[derive(Debug)]
pub struct Table(BTreeMap<String, [String; 2]>);

impl Table {
    /// The cell of row `id` for operating system `os` (0 Windows, 1 macOS).
    pub fn cell(&self, id: &str, os: usize) -> Option<&str> {
        self.0.get(id).and_then(|cells| cells.get(os)).map(String::as_str)
    }
}

/// Split `| a | b \| c | d |` into cells, with `\|` read as a pipe inside a cell.
fn split_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner
        .strip_suffix('|')
        .filter(|rest| !rest.ends_with('\\'))
        .unwrap_or(inner);
    let mut cells = vec![String::new()];
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                if let Some(cell) = cells.last_mut() {
                    cell.push('|');
                }
            }
            '|' => cells.push(String::new()),
            other => {
                if let Some(cell) = cells.last_mut() {
                    cell.push(other);
                }
            }
        }
    }
    cells.into_iter().map(|c| c.trim().to_owned()).collect()
}

fn is_separator(cells: &[String]) -> bool {
    cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
}

fn is_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .filter(|s| s.bytes().all(|d| d.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    matches!(
        (number(0..4), number(5..7), number(8..10)),
        (Some(_), Some(1..=12), Some(1..=31))
    )
}

/// Whether `value` is a valid cell of a row of this kind. `NOT RUN` is valid everywhere; nothing else is, unless it fits.
fn check_cell(kind: Kind, value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("the cell is empty; write NOT RUN if the check was not run");
    }
    if value.chars().count() > MAX_CELL_CHARS {
        return Err("the cell is longer than 200 characters");
    }
    if value.chars().any(char::is_control) {
        return Err("the cell holds a control character");
    }
    if value == NOT_RUN {
        return Ok(());
    }
    if value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .eq_ignore_ascii_case(NOT_RUN)
    {
        return Err("write NOT RUN exactly, in capitals with one space");
    }
    match kind {
        Kind::Check => match value {
            "PASS" | "FAIL" => Ok(()),
            _ if value
                .strip_prefix("FAIL: ")
                .is_some_and(|reason| !reason.trim().is_empty()) =>
            {
                Ok(())
            }
            _ => Err("a check is NOT RUN, PASS, FAIL or `FAIL: reason`"),
        },
        Kind::Date if !is_date(value) => Err("the date is YYYY-MM-DD"),
        Kind::Decision(setting) if !setting.allowed().contains(&value) => {
            Err("not one of the documented values of the setting")
        }
        Kind::Date | Kind::Decision(_) | Kind::CursorBuild | Kind::Person | Kind::Observation => Ok(()),
    }
}

/// Read the results table out of the `- **D91**` entry of `docs/decisions.md`. Only that entry's rows count: the same ids in
/// another entry are ignored.
pub fn parse(text: &str) -> Result<Table, StatusError> {
    let mut inside = false;
    let mut found = false;
    let mut cells: BTreeMap<String, [String; 2]> = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with("- **D") || line.starts_with("## ") {
            inside = line.starts_with("- **D91**");
            found |= inside;
            continue;
        }
        let line = line.trim_start();
        if !inside || !line.starts_with('|') {
            continue;
        }
        let row = split_row(line);
        let id = row
            .first()
            .map(|c| c.trim_matches('`').to_owned())
            .unwrap_or_default();
        if id.eq_ignore_ascii_case("ID") || is_separator(&row) {
            continue;
        }
        let id: String = id.chars().take(40).collect();
        let [_, _, windows, macos] =
            <[String; 4]>::try_from(row).map_err(|_| StatusError::BadRow(id.clone()))?;
        let Some(spec) = ROWS.iter().find(|spec| spec.id == id) else {
            return Err(StatusError::UnknownRow(id));
        };
        for (os, value) in OS.into_iter().zip([&windows, &macos]) {
            check_cell(spec.kind, value).map_err(|why| StatusError::BadCell {
                row: id.clone(),
                os,
                why,
            })?;
        }
        if cells.insert(id.clone(), [windows, macos]).is_some() {
            return Err(StatusError::DuplicateRow(id));
        }
    }
    if !found {
        return Err(StatusError::NoD91);
    }
    if let Some(spec) = ROWS.iter().find(|spec| !cells.contains_key(spec.id)) {
        return Err(StatusError::MissingRow(spec.id.to_owned()));
    }
    Ok(Table(cells))
}

/// What the table says, per operating system.
#[derive(Debug, Default)]
pub struct Report {
    pub recorded: [usize; 2],
    pub pass: [usize; 2],
    pub fail: [usize; 2],
    pub not_run: [usize; 2],
    /// Recorded cells that break a rule (not NOT RUN cells, which are counted above).
    pub problems: Vec<String>,
}

pub fn report(table: &Table) -> Report {
    let mut report = Report::default();
    for (os, name) in OS.into_iter().enumerate() {
        let value = |id: &str| table.cell(id, os).unwrap_or(NOT_RUN);
        for spec in ROWS {
            let cell = value(spec.id);
            if cell == NOT_RUN {
                report.not_run[os] += 1;
                continue;
            }
            report.recorded[os] += 1;
            if spec.kind == Kind::Check {
                if cell == "PASS" {
                    report.pass[os] += 1;
                } else {
                    report.fail[os] += 1;
                }
            }
        }
        let build = value("META.cursor_build");
        if build != NOT_RUN && build != CURSOR_BUILD && !build.starts_with(&format!("{CURSOR_BUILD} ")) {
            report.problems.push(format!("{name}: META.cursor_build is not {CURSOR_BUILD}, so these results are not about the build T0 is for"));
        }
        let failed = |ids: &[&str]| ids.iter().any(|id| value(id).starts_with("FAIL"));
        let oauth_failed = failed(&["C1.discovery", "C1.entra_accepts", "C1.tool_call"]);
        if oauth_failed && value("C1.headers").starts_with("FAIL") {
            // Prompt 07, "Stop and escalate if": neither OAuth nor headers work reliably in 3.22.12.
            report.problems.push(format!(
                "{name}: neither OAuth nor headers work, so stop and escalate (prompt 07)"
            ));
        } else if oauth_failed && value("D.auth") != "personal_token" {
            report
                .problems
                .push(format!("{name}: OAuth failed, so D.auth must be personal_token"));
        }
        if failed(&["C2.prompt", "C2.accept", "C2.decline", "C2.cancel", "C2.no_hang"])
            && value("D.confirm") != "draft"
        {
            report
                .problems
                .push(format!("{name}: elicitation failed, so D.confirm must be draft"));
        }
    }
    report
}

fn shown(value: &str) -> String {
    if value.chars().count() > SHOWN_CHARS {
        let cut: String = value.chars().take(SHOWN_CHARS).collect();
        format!("{cut}...")
    } else {
        value.to_owned()
    }
}

/// Print the status. Returns the text and the exit code.
pub fn run(text: &str, strict: bool) -> (String, i32) {
    let table = match parse(text) {
        Ok(table) => table,
        Err(error) => {
            return (
                format!("t0_status: cannot read D91 in docs/decisions.md: {error}\n"),
                1,
            );
        }
    };
    let report = report(&table);
    let total = ROWS.len();
    let recorded: usize = report.recorded.iter().sum();
    let not_run: usize = report.not_run.iter().sum();
    let overall = if recorded == 0 {
        "NOT RUN"
    } else if not_run > 0 {
        "INCOMPLETE"
    } else {
        "RECORDED"
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "T0, the Cursor {CURSOR_BUILD} compatibility test. Source: D91 in docs/decisions.md"
    );
    let _ = writeln!(out, "overall: {overall}");
    for (os, name) in OS.into_iter().enumerate() {
        let _ = writeln!(
            out,
            "  {name}: {} of {total} cells recorded (PASS {}, FAIL {}, NOT RUN {})",
            report.recorded[os], report.pass[os], report.fail[os], report.not_run[os]
        );
    }
    let _ = writeln!(out, "rows ({} | {}):", OS[0], OS[1]);
    for spec in ROWS {
        let (w, m) = (
            table.cell(spec.id, 0).unwrap_or(NOT_RUN),
            table.cell(spec.id, 1).unwrap_or(NOT_RUN),
        );
        let _ = writeln!(out, "  {:<26} {} | {}", spec.id, shown(w), shown(m));
    }
    if !report.problems.is_empty() {
        let _ = writeln!(out, "problems:");
        for problem in &report.problems {
            let _ = writeln!(out, "  - {problem}");
        }
    }
    let _ = writeln!(
        out,
        "NOT RUN is not a pass. An agent cannot run Cursor {CURSOR_BUILD} and never fills a cell; a human does, on Windows and on macOS (crates/hub-mcp/t0/README.md)."
    );

    let mut code = 0;
    if strict {
        let mut reasons = Vec::new();
        for (name, n) in OS.into_iter().zip(report.not_run) {
            if n > 0 {
                reasons.push(format!(
                    "{name}: {n} {} NOT RUN",
                    if n == 1 { "cell is" } else { "cells are" }
                ));
            }
        }
        reasons.extend(report.problems.iter().cloned());
        if reasons.is_empty() {
            let _ = writeln!(out, "strict: ok");
        } else {
            let _ = writeln!(out, "strict: FAILED");
            for reason in &reasons {
                let _ = writeln!(out, "  - {reason}");
            }
            code = 1;
        }
    }
    (out, code)
}

fn default_decisions_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/decisions.md")
}

fn main() -> ExitCode {
    let mut strict = false;
    let mut path = default_decisions_path();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--strict" => strict = true,
            "--decisions" => match args.next() {
                Some(value) => path = PathBuf::from(value),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        let _ = writeln!(std::io::stderr(), "t0_status: cannot read the decisions file");
        return ExitCode::from(1);
    };
    let (output, code) = run(&text, strict);
    let _ = std::io::stdout().write_all(output.as_bytes());
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn usage() -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "usage: t0_status [--strict] [--decisions <path to docs/decisions.md>]"
    );
    ExitCode::from(2)
}
