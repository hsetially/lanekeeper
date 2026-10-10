//! The agent binary (T1, T7): read the environment, open the NFS root, log in JSON, and run until told to stop.
//!
//! Exit codes: 0 after an orderly shutdown, 1 when the agent stopped because something failed, 2 when it could not
//! start because the environment is wrong. The kubelet restarts the container on anything but 0.
#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::Duration;

use agent::config::ProcessEnv;
use agent::ops::{Metrics, logging};
use tokio::signal::unix::{SignalKind, signal};
use tracing::error;

/// Threads that run async code. The agent is I/O-light: heartbeats, a few commands and the Kubernetes watches.
const RUNTIME_WORKERS: usize = 2;
/// Threads for blocking file work (reads, writes, walks). Bounded (rule 5); the scan pool is separate.
const BLOCKING_THREADS: usize = 16;
/// A signal handler that cannot be installed leaves only this: wait for ctrl-c.
const NO_SIGNALS: Duration = Duration::from_secs(1);

/// Resolves on SIGTERM (what the kubelet sends) or SIGINT.
async fn shutdown_signal() {
    let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt()))
    else {
        // Without handlers the process is still stopped by SIGKILL after the grace period; say so and wait.
        tokio::time::sleep(NO_SIGNALS).await;
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[allow(
    clippy::print_stderr,
    reason = "before the logger exists, and when it cannot be installed, stderr is the only place to say why"
)]
fn main() -> ExitCode {
    let started = match agent::startup(&ProcessEnv) {
        Ok(started) => started,
        Err(error) => {
            // Errors name variables and paths, never values (S10), so this line is safe to print and to ship.
            eprintln!("agent: cannot start: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = logging::init(started.settings.log_level) {
        eprintln!("agent: cannot start the logger: {error}");
        return ExitCode::from(2);
    }
    // One crypto provider for every TLS user in the process (the hub connection builds its own config; the Kubernetes
    // client takes the process default).
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        error!("another crypto provider was installed first");
        return ExitCode::from(2);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKERS)
        .max_blocking_threads(BLOCKING_THREADS)
        .thread_name("lk-agent")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("agent: cannot start the runtime: {:?}", error.kind());
            return ExitCode::from(2);
        }
    };
    let metrics = Metrics::new();
    match runtime.block_on(agent::process::run(started, metrics, shutdown_signal())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            error!(%failure, "the agent stopped");
            ExitCode::FAILURE
        }
    }
}
