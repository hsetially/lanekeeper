//! A hub that serves only the built web app, through the production header code, on loopback (prompt 08, T1).
//!
//! The web app's CSP, performance and visual tests need the real `Content-Security-Policy` of `index.html`: the nonce
//! substitution of `StaticFiles`, `headers::html_csp` with `require-trusted-types-for 'script'` and the Trusted Types names
//! of `LK_TRUSTED_TYPES`, `Cache-Control`, HSTS and the rest of `security_headers`. This example composes a `Hub` with
//! `Hub::build` (no modules, so no API route) and serves `Hub::web` on a plain-HTTP loopback socket. It is a test tool
//! and is not part of the image. The API itself is answered by the Node harness in `web/e2e/harness`, which proxies
//! every other path here untouched.
//!
//! Environment (all optional): `LK_WEB_DIST` (default `web/dist`), `LK_FIXTURE_ADDR` (default `127.0.0.1:0`, loopback
//! only) and any `LK_*` setting the hub reads (for example `LK_TRUSTED_TYPES`). The per-IP limiter would answer 429 to an
//! asset sweep or a Lighthouse run from one address, so its rate is raised here and the trusted proxy hops are 0.
//!
//! On start it prints one line, `listening http://127.0.0.1:PORT`, and flushes it.

use std::io::Write;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use hub::serve::{ServeLimits, serve};
use hub::static_files::StaticFiles;
use hub::{Deps, Hub, HubSettings};
use hub_platform::FakeSessionAuth;
use tokio::net::TcpListener;

/// The settings of the fixture: the web root, a limiter that does not answer 429 to an asset sweep from one address, and
/// no trusted proxy. Every other `LK_*` name is read from the environment, as in a deployment.
fn lookup(name: &str) -> Option<String> {
    match name {
        "LK_IP_RATE_PER_SEC" | "LK_IP_BURST" => Some("100000".to_owned()),
        "LK_TRUSTED_PROXY_HOPS" => Some("0".to_owned()),
        "LK_WEB_DIST" => Some(std::env::var("LK_WEB_DIST").unwrap_or_else(|_| "web/dist".to_owned())),
        other => std::env::var(other).ok(),
    }
}

/// Wait for a stop signal.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn fail(message: &str) -> ExitCode {
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "web_fixture: {message}");
    ExitCode::FAILURE
}

#[tokio::main]
async fn main() -> ExitCode {
    // `ConfigError` names the variable and never prints its value.
    let settings = match HubSettings::from_lookup(lookup) {
        Ok(settings) => settings,
        Err(error) => return fail(&error.to_string()),
    };
    let Ok(files) = StaticFiles::open(&settings.web_dist) else {
        return fail(
            "the web root has no index.html with the nonce placeholder (run `pnpm build` in web/ first)",
        );
    };
    let addr: SocketAddr = match std::env::var("LK_FIXTURE_ADDR") {
        Ok(text) => match text.parse() {
            Ok(addr) => addr,
            Err(_) => return fail("LK_FIXTURE_ADDR is not an address"),
        },
        Err(_) => SocketAddr::from(([127, 0, 0, 1], 0)),
    };
    if !addr.ip().is_loopback() {
        return fail("the fixture listens on loopback only");
    }
    let Ok(hub) = Hub::build(Deps {
        settings,
        auth: Arc::new(FakeSessionAuth::new()),
        modules: Vec::new(),
        static_files: Some(Arc::new(files)),
        checks: Vec::new(),
    }) else {
        return fail("the hub could not be composed");
    };
    let Ok(listener) = TcpListener::bind(addr).await else {
        return fail("could not bind the address");
    };
    let Ok(bound) = listener.local_addr() else {
        return fail("could not read the bound address");
    };
    let mut stdout = std::io::stdout();
    if writeln!(stdout, "listening http://{bound}").is_err() || stdout.flush().is_err() {
        return fail("could not write the announcement");
    }
    serve(listener, None, hub.web, stopped(), ServeLimits::default()).await;
    ExitCode::SUCCESS
}
