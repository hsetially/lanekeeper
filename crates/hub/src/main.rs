//! The hub process.
//!
//! The binary starts only when it has a real identity module (03a) and a real registry (05): it never falls back to a fake,
//! because a fake session store in production would let anyone in. Until those crates land, it reads and checks its settings
//! and then stops with that reason. The integration tests build hubs with `hub::Hub::build` and the fakes instead.

use std::process::ExitCode;

use hub::HubSettings;

fn main() -> ExitCode {
    let settings = match HubSettings::from_lookup(|name| std::env::var(name).ok()) {
        Ok(settings) => settings,
        Err(error) => {
            // The settings are not read, so telemetry is not up yet; stderr is the only place left.
            #[allow(clippy::print_stderr)]
            {
                eprintln!("hub: {error}");
            }
            return ExitCode::from(2);
        }
    };
    let _telemetry = match hub::telemetry::init(&settings) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            #[allow(clippy::print_stderr)]
            {
                eprintln!("hub: {error}");
            }
            return ExitCode::from(2);
        }
    };
    tracing::error!(
        "this build has no identity module (prompt 03a) and no registry (prompt 05) linked in; refusing to start without them"
    );
    ExitCode::from(1)
}
