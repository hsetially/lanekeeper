//! Developer tasks. Owned by prompt 01 (T8): `gen-fixtures`, `bench-check`.
#![forbid(unsafe_code)]
#![allow(clippy::print_stderr, clippy::print_stdout)] // a CLI tool prints by design

use std::process::ExitCode;

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "gen-fixtures" | "bench-check" => {
            eprintln!("xtask {task}: not implemented yet, see prompts/01-contracts.md T8");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("usage: cargo xtask <gen-fixtures|bench-check>");
            ExitCode::FAILURE
        }
    }
}
