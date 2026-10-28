//! The hub process.
//!
//! `hub` (or `hub serve`) starts the hub and serves until SIGTERM or SIGINT. It starts only on real components, and when one
//! is missing it says which and exits; it never falls back to a fake, because a fake session store in production would let
//! anyone in. Everything it does is in `hub::process`, which the dev example shares: this file chooses the real edges.
//!
//! Exit codes: 0 stopped on request, 1 could not start (or told twice), 2 the command line or a setting was refused.

use std::process::ExitCode;

use hub::edges::Edges;

fn main() -> ExitCode {
    hub::process::entry(
        // Arguments that are not UTF-8 are mangled, not trusted: they will not match a command and the usage is printed.
        std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned()),
        |name| std::env::var(name).ok(),
        Edges::google,
    )
}
