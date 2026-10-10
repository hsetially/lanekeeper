//! The agent binary (T1): read the environment, open the NFS root, and stop with a clear error if either is wrong.
//!
//! The session loop arrives with T3 and T7. Until then the binary only proves that it can start.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use agent::config::ProcessEnv;

#[allow(
    clippy::print_stderr,
    reason = "no logger exists yet when the settings cannot be read; structured logging arrives with T7"
)]
fn main() -> ExitCode {
    match agent::startup(&ProcessEnv) {
        Ok(_started) => ExitCode::SUCCESS,
        Err(error) => {
            // Errors name variables and paths, never values (S10), so this line is safe to print and to ship.
            eprintln!("agent: cannot start: {error}");
            ExitCode::from(2)
        }
    }
}
