//! The hub with software edges, for the process tests and for `just dev` (plan 18, T1 to T8, G23).
//!
//! The real binary cannot start without Google: its secrets come from Secret Manager through the workload's identity. This
//! example runs the same `hub::process::entry`, and so the same start, serve and drain code, with the one difference that the
//! secrets are files in a directory, `LK_DEV_SECRETS_DIR`: the secret named `web-cert` is the file `web-cert` there. The CA keys
//! are software keys made at start (`soft_kms`), and the CA certificates are made for them by `hub::ca_init::run`, the same
//! ceremony `hub ca-init` runs, and written into that directory under the names `LK_AGENT_CA_CERT_REF` and
//! `LK_REPLICA_CA_CERT_REF` give. No Google ID token is accepted: the verifier knows no token. The write paths' two edges (T5) are
//! software too: a KMS envelope whose key the process holds, and GitHub's client pointed at `LK_DEV_GITHUB_API`, a plain-HTTP
//! server of the caller's (nothing listens at the default, so every GitHub call fails the way a GitHub that is down does). The MCP
//! server's one secret, the key that hashes personal tokens, is made at the first start if it is not there. It is an
//! example target, so it is never part of the image or of a release build of `hub`; `tests/release_build.rs` keeps it that way.
//!
//! Settings are the hub's own `LK_*` variables. `LK_DEV_SECRETS_DIR`, `LK_DEV_GITHUB_API` and `LK_DEV_GOOGLE_TOKENS` (comma-separated
//! `token=service-account-email` pairs: the Google ID tokens an agent may join with, for the audience `LK_HUB_AUDIENCE`) are the
//! additions and are read in `support/dev_edges.rs`, not by the hub. `LK_DEV_LOGIN_ADDR` (a loopback address) starts the development
//! sign-in of `support/dev_login.rs`, which is how a person at a keyboard gets a session, since a browser cannot sign in to a mock Entra.

#[path = "support/dev_edges.rs"]
mod dev_edges;
#[path = "support/dev_login.rs"]
mod dev_login;
#[path = "support/soft_kms.rs"]
mod soft_kms;

use std::process::ExitCode;

#[allow(clippy::print_stderr)]
fn main() -> ExitCode {
    if let Err(error) = dev_login::spawn_if_asked(&|name| std::env::var(name).ok()) {
        eprintln!("hub_dev: {error}");
        return ExitCode::from(2);
    }
    hub::process::entry(
        std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned()),
        |name| std::env::var(name).ok(),
        |settings| dev_edges::dev_edges(settings, &|name| std::env::var(name).ok()),
    )
}
