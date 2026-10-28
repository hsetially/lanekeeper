//! The whole hub, started in the test's own process by `hub::process::start` (plan 18, T8): the same start the binary and the
//! `hub_dev` example run, over a real Postgres 16 with TLS, with the software edges of `examples/support/dev_edges.rs`.
//!
//! The tests that read the route table (`Running::routes()`, which a child process cannot hand over) and the tests that send the
//! composed hub a request of every kind need the real thing in reach, and this is it. Nothing is faked inside: every module is
//! built, every listener is bound on port 0, every job is registered. The including test declares `browser`, `child`, `git_remote`, `net`,
//! `rig`, `support`, `tls_pg`, `soft_kms` and `dev_edges` at its root, like `tests/process.rs`.
//!
//! One thing differs from the child process: the Git mirrors cannot fetch from the rig's HTTPS remote, because the rig's CA reaches
//! `gix` through `SSL_CERT_FILE` and a test cannot set an environment variable (the workspace forbids `unsafe`). So `/readyz` stays
//! 503 on `mirrors`; nothing here reads it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;

use domain::{Role, UserId, UserStatus};
use hub::HubSettings;
use hub::process::Running;
use sqlx::PgPool;

use crate::browser::{Browser, Cookies, ORIGIN};
use crate::dev_edges::dev_edges;
use crate::net;
use crate::rig::Rig;

/// A rig and the hub running on it.
pub struct Full {
    pub rig: Rig,
    pub running: Running,
    /// The database as the hub's own role, for the users and sessions a test makes the way sign-in would have.
    pub pool: PgPool,
    browser: Browser,
}

impl Full {
    /// `None` when Docker is not there and not required (`LK_REQUIRE_DOCKER=1` makes that a failure).
    pub async fn start() -> Option<Self> {
        Self::start_with(&[]).await
    }

    /// Like [`Full::start`], with settings added to or replaced in the rig's.
    pub async fn start_with(extra: &[(&str, &str)]) -> Option<Self> {
        let rig = Rig::start().await?;
        let env: Arc<HashMap<String, String>> = Arc::new(rig.env(extra).into_iter().collect());
        let lookup = {
            let env = Arc::clone(&env);
            move |name: &str| env.get(name).cloned()
        };
        let settings = HubSettings::from_lookup(&lookup).expect("the rig's settings are valid");
        let edges_settings = HubSettings::from_lookup(&lookup).expect("the rig's settings are valid");
        hub::process::install_crypto_provider();
        // The software KMS makes the CA certificates with a `block_on` of its own, which a thread of the runtime may not do.
        let edges = {
            let lookup = lookup.clone();
            tokio::task::spawn_blocking(move || dev_edges(&edges_settings, &lookup))
                .await
                .unwrap()
                .expect("the software edges")
        };
        let running = hub::process::start(settings, edges)
            .await
            .expect("the hub starts");
        let pool = rig.pg.app_pool().await;
        Some(Self {
            rig,
            running,
            browser: Browser::new(pool.clone()),
            pool,
        })
    }

    /// A user the way sign-in would have made one, and a session for them. The hub finds the session in the table on its first
    /// request, as it does for one that another replica made.
    pub async fn signed_in(&self, n: u32, role: Option<Role>, status: UserStatus) -> (UserId, Cookies) {
        self.browser.signed_in(n, role, status).await
    }

    /// A request as a page makes it: the cookies, and for a state-changing method the double-submit token and the `Origin`.
    pub async fn web_as(
        &self,
        cookies: &Cookies,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> net::Reply {
        let cookie = cookies.header();
        let mut headers: Vec<(&str, &str)> = vec![("Cookie", &cookie)];
        if method != "GET" {
            headers.push(("X-CSRF-Token", &cookies.csrf));
            headers.push(("Origin", ORIGIN));
        }
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let text = body.map(ToString::to_string);
        self.web(method, path, &headers, text.as_deref().map(str::as_bytes))
            .await
    }

    /// One request to the web listener over TLS 1.3, as a client that trusts the rig's CA and presents no cookie.
    pub async fn web(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> net::Reply {
        net::https_request(
            self.running.web_addr(),
            &self.rig.ca.cert_der(),
            method,
            path,
            headers,
            body,
        )
        .await
    }
}

/// A path of the table with its parameters filled, for a request that only has to reach the route.
pub fn concrete(template: &str) -> String {
    template
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "x1"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}
