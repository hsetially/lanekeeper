//! The edges of the process (plan 18, T1): the ports that reach outside it.
//!
//! Everything the hub does that leaves the process goes through a port in this struct, so a test fills the struct with
//! software and the real binary fills it with Google. No fake is compiled into the `hub` binary: `Edges::google` is the only
//! constructor outside tests and examples, and `GoogleEndpoints::production()` is the only value it uses. Each task adds the
//! edges it needs (T3 the KMS signer and envelope and the Google ID token verifier, T4 identity's HTTPS client and the
//! checkpoint bucket, T5 GitHub); T1 needs the secret source and the clock.

use std::fmt;
use std::sync::Arc;

use hub_platform::clock::{Clock, SystemClock};
use hub_platform::gcp::http::{GoogleHttp, HttpLimits};
use hub_platform::gcp::secret_manager::{DEFAULT_CACHE_ENTRIES, MAX_CACHE_TTL};
use hub_platform::gcp::token::WorkloadIdentityTokens;
use hub_platform::gcp::{
    CallLimits, GoogleApi, GoogleEndpoints, GoogleMetrics, SecretManagerSource, Transport,
};
use ports::SecretSource;

use crate::config::{ConfigError, HubSettings};
use crate::process::StartError;

/// The ports that reach outside the process.
#[derive(Clone)]
pub struct Edges {
    /// Runtime secrets (S7): Secret Manager in production.
    pub secrets: Arc<dyn SecretSource>,
    /// Wall time for instants that go into data and into expiry checks. Waiting uses tokio's clock.
    pub clock: Arc<dyn Clock>,
    /// The Google clients' counters, registered on the hub's metrics. Empty when no Google client exists.
    pub google_metrics: GoogleMetrics,
}

impl fmt::Debug for Edges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Edges").finish_non_exhaustive()
    }
}

impl Edges {
    /// Edges from parts the caller made: software in tests and in the dev example.
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretSource>, clock: Arc<dyn Clock>) -> Self {
        Self {
            secrets,
            clock,
            google_metrics: GoogleMetrics::default(),
        }
    }

    /// The real edges: Secret Manager through the workload's identity, and the system clock. It makes no call; the first one
    /// is the first secret read, under the start-up timeout.
    pub fn google(settings: &HubSettings) -> Result<Self, StartError> {
        let gcp = settings
            .gcp
            .as_ref()
            .ok_or(StartError::Config(ConfigError::Missing("LK_GCP_PROJECT")))?;
        let endpoints = GoogleEndpoints::production();
        let http = GoogleHttp::production(HttpLimits::default()).map_err(|_| StartError::Google)?;
        let google_metrics = GoogleMetrics::default();
        let transport = Transport::new(http, CallLimits::default(), google_metrics.clone());
        let tokens = Arc::new(WorkloadIdentityTokens::new(
            transport.clone(),
            endpoints.metadata.clone(),
        ));
        let api = GoogleApi::new(transport, tokens);
        let secrets = SecretManagerSource::new(
            api,
            endpoints.secret_manager,
            gcp.project.clone(),
            MAX_CACHE_TTL,
            DEFAULT_CACHE_ENTRIES,
        );
        Ok(Self {
            secrets: Arc::new(secrets),
            clock: Arc::new(SystemClock),
            google_metrics,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> HubSettings {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        HubSettings::from_lookup(move |name| map.get(name).cloned()).unwrap()
    }

    #[test]
    fn the_real_edges_need_a_google_project_and_say_which_variable() {
        let error = Edges::google(&settings(&[])).unwrap_err();
        assert_eq!(error, StartError::Config(ConfigError::Missing("LK_GCP_PROJECT")));
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn the_real_edges_are_built_without_a_call() {
        // Building the clients reads the platform's root certificates and nothing else: no network, no credential.
        let edges = Edges::google(&settings(&[("LK_GCP_PROJECT", "lanekeeper-prod")]));
        assert!(edges.is_ok(), "{edges:?}");
    }
}
