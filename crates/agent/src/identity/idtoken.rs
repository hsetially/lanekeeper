//! The Google ID token for Workload Identity (S5, Q26).

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Secret, ShortText};

use super::error::IdTokenError;
use crate::config::BaseUrl;
use crate::http::{HttpClient, HttpError, HttpRequest};

/// Where an ID token for an audience comes from.
#[async_trait]
pub trait IdTokenSource: Send + Sync + fmt::Debug {
    /// A Google-signed ID token for the service account of this workload, for `audience`.
    async fn id_token(&self, audience: &ShortText) -> Result<Secret<String>, IdTokenError>;
}

/// The GCE metadata server, which on GKE with Workload Identity answers for the pod's own service account.
#[derive(Debug, Clone)]
pub struct MetadataIdTokens {
    http: HttpClient,
    base: BaseUrl,
}

/// How long the metadata server gets to answer. It is on the node, so a slow answer means there is none.
const TIMEOUT: Duration = Duration::from_secs(5);
/// A token is at most 8 KiB (the contract's limit); the metadata server's answer is nothing but the token.
const MAX_BODY: usize = 16 * 1024;
/// The longest token the hub accepts (`proto::limits`).
const MAX_TOKEN: usize = proto::limits::MAX_TOKEN_BYTES;

impl MetadataIdTokens {
    pub fn new(base: BaseUrl) -> Self {
        Self::with_client(HttpClient::new(TIMEOUT, MAX_BODY), base)
    }

    /// With a client of the caller's choosing (a test uses a shorter timeout).
    pub fn with_client(http: HttpClient, base: BaseUrl) -> Self {
        Self { http, base }
    }
}

#[async_trait]
impl IdTokenSource for MetadataIdTokens {
    async fn id_token(&self, audience: &ShortText) -> Result<Secret<String>, IdTokenError> {
        let target = format!(
            "/computeMetadata/v1/instance/service-accounts/default/identity?audience={}",
            percent_encode(audience.as_str())
        );
        let request = HttpRequest::get(target)
            .with_header("Metadata-Flavor", "Google")
            .map_err(|_| IdTokenError::Malformed)?;
        let response = self.http.send(&self.base, request).await.map_err(|e| match e {
            // Nothing answered: this is "no Workload Identity here", which the join may fall back from.
            HttpError::Connect | HttpError::Timeout => IdTokenError::Unavailable,
            HttpError::TooLarge | HttpError::Protocol | HttpError::InvalidRequest => IdTokenError::Malformed,
        })?;
        if response.status != 200 {
            return Err(IdTokenError::Refused {
                status: response.status,
            });
        }
        let text = std::str::from_utf8(&response.body).map_err(|_| IdTokenError::Malformed)?;
        let token = text.trim_end_matches(['\r', '\n']);
        if !looks_like_a_jwt(token) {
            return Err(IdTokenError::Malformed);
        }
        Ok(Secret::new(token.to_owned()))
    }
}

/// Three non-empty base64url segments, as in every compact JWS, within the contract's size limit. Public for the
/// fuzz target `agent_id_token`: the text comes from the metadata server, outside the process.
pub fn looks_like_a_jwt(token: &str) -> bool {
    let segment_ok = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    token.len() <= MAX_TOKEN && {
        let mut parts = token.split('.');
        matches!(
            (parts.next(), parts.next(), parts.next(), parts.next()),
            (Some(a), Some(b), Some(c), None) if segment_ok(a) && segment_ok(b) && segment_ok(c)
        )
    }
}

/// Percent-encode everything except the RFC 3986 unreserved characters, so the audience is one query value.
fn percent_encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("abc-._~XYZ019"), "abc-._~XYZ019");
        assert_eq!(
            percent_encode("https://hub.example.com"),
            "https%3A%2F%2Fhub.example.com"
        );
        assert_eq!(percent_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(percent_encode("é"), "%C3%A9");
        assert_eq!(percent_encode(""), "");
    }
}
