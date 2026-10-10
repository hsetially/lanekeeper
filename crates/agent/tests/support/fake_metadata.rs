//! A fake GCE metadata server for the Workload Identity path (S5, Q26): it serves an ID token to requests that carry
//! `Metadata-Flavor: Google`, and can be made to fail in the ways a real one does.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::raw_server::{RawServer, RecordedRequest, Reply, unreachable_url};

pub struct FakeMetadata {
    server: Option<RawServer>,
    url: String,
}

impl FakeMetadata {
    /// Serves `token` to any request that has the flavor header, and `403` to one that has not (as the real one does).
    pub async fn serving(token: &str) -> Self {
        let token = token.to_owned();
        Self::from(
            RawServer::start(move |request| {
                if request.header("metadata-flavor") == Some("Google") {
                    Reply::ok(token.clone())
                } else {
                    Reply::status(403, "Missing Metadata-Flavor:Google header.")
                }
            })
            .await,
        )
    }

    /// Answers every request with `status`.
    pub async fn failing_with(status: u16) -> Self {
        Self::from(RawServer::start(move |_| Reply::status(status, "error")).await)
    }

    /// Accepts connections and never answers.
    pub async fn hanging() -> Self {
        Self::from(RawServer::start(|_| Reply::Hang).await)
    }

    /// Nothing listens, as on a cluster without Workload Identity.
    pub async fn absent() -> Self {
        Self {
            server: None,
            url: unreachable_url().await,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The requests that reached it.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.server.as_ref().map(RawServer::requests).unwrap_or_default()
    }
}

impl From<RawServer> for FakeMetadata {
    fn from(server: RawServer) -> Self {
        Self {
            url: server.url(),
            server: Some(server),
        }
    }
}
