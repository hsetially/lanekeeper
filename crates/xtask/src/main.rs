//! Developer tasks. Owned by prompt 01 (T8): `gen-fixtures`, `bench-check`.
#![forbid(unsafe_code)]
#![allow(clippy::print_stderr, clippy::print_stdout)] // a CLI tool prints by design

use std::process::ExitCode;

use xtask::cli;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (task, rest) = args.split_first().map_or(("", &[][..]), |(t, r)| (t.as_str(), r));
    match task {
        "bench-check" => {
            let (text, code) = cli::run_bench_check(rest);
            if code == cli::EXIT_USAGE {
                eprint!("{text}");
            } else {
                print!("{text}");
            }
            ExitCode::from(code)
        }
        "gen-fixtures" => {
            let (text, code) = cli::run_gen_fixtures(rest);
            if code == cli::EXIT_OK {
                print!("{text}");
            } else {
                eprint!("{text}");
            }
            ExitCode::from(code)
        }
        _ => {
            eprintln!("{}", cli::USAGE);
            ExitCode::from(cli::EXIT_USAGE)
        }
    }
}
