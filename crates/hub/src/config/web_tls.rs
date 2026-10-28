//! The web listener's certificate (plan 18, T1, G3, S6, S7): a chain and a key in two Secret Manager secrets, named here.
//!
//! The settings hold names, never PEM text. The certificate and the key are read at start and again on a timer by
//! `crate::web_tls`; they are never a Kubernetes Secret and never a variable.

use std::time::Duration;

use super::ConfigError;
use super::reader::Reader;

/// How often the certificate is read again unless `LK_WEB_CERT_RELOAD_SECS` says otherwise (G3).
pub const DEFAULT_RELOAD_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTlsSettings {
    /// The secret that holds the certificate chain, leaf first, as PEM.
    pub cert_ref: String,
    /// The secret that holds the private key, as PEM.
    pub key_ref: String,
    /// The time between two reads of the secrets. Each wait is jittered, so replicas do not read in step.
    pub reload_every: Duration,
}

impl WebTlsSettings {
    /// Both names or neither: a certificate without its key (or the reverse) is a half-written deployment, not a default.
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let (cert_ref, key_ref) = (
            reader.secret_ref("LK_WEB_CERT_REF")?,
            reader.secret_ref("LK_WEB_KEY_REF")?,
        );
        let reload = reader.number("LK_WEB_CERT_RELOAD_SECS", DEFAULT_RELOAD_SECS, 1, 86_400)?;
        match (cert_ref, key_ref) {
            (None, None) => Ok(None),
            (Some(cert_ref), Some(key_ref)) => Ok(Some(Self {
                cert_ref,
                key_ref,
                reload_every: Duration::from_secs(reload),
            })),
            (Some(_), None) => Err(ConfigError::Missing("LK_WEB_KEY_REF")),
            (None, Some(_)) => Err(ConfigError::Missing("LK_WEB_CERT_REF")),
        }
    }
}
