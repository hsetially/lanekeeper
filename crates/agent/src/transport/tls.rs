//! TLS toward the hub (S6): TLS 1.3 and nothing older, one pinned CA, and a client certificate only when there is one.
//!
//! The agent builds its own rustls configuration instead of using tonic's, so that these three things are written
//! in one place and cannot be loosened by a feature flag. The only trust anchors are the certificates in
//! `LK_HUB_CA_FILE`: the system roots are never consulted, so a certificate for the hub's name from any public CA is
//! refused.

use std::fmt;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use rustls::client::{ResolvesClientCert, Resumption};
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, RootCertStore, SignatureScheme};

use super::error::{TlsError, TlsFailure};
use crate::identity::ClientIdentity;

/// The ALPN name for HTTP/2, which gRPC needs.
pub const ALPN_H2: &[u8] = b"h2";

/// The hub CA file is a few kilobytes. Anything bigger is not a CA file.
pub const MAX_CA_FILE_BYTES: usize = 256 * 1024;
/// A root, plus an intermediate or two while a CA is being rotated.
pub const MAX_CA_CERTIFICATES: usize = 8;

const PEM_CERTIFICATE: &str = "CERTIFICATE";

/// The trust anchors for the hub's certificate: exactly what the operator pinned.
#[derive(Clone)]
pub struct HubRoots {
    store: Arc<RootCertStore>,
}

impl fmt::Debug for HubRoots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubRoots")
            .field("anchors", &self.store.roots.len())
            .finish()
    }
}

impl HubRoots {
    /// Read the PEM text of `LK_HUB_CA_FILE`: one to [`MAX_CA_CERTIFICATES`] `CERTIFICATE` blocks and nothing else.
    pub fn from_pem(text: &[u8]) -> Result<Self, TlsError> {
        let blocks = pem::parse_many(text).map_err(|_| TlsError::CaMalformed)?;
        if blocks.is_empty() {
            return Err(TlsError::CaEmpty);
        }
        if blocks.len() > MAX_CA_CERTIFICATES {
            return Err(TlsError::CaTooMany);
        }
        let mut store = RootCertStore::empty();
        for block in blocks {
            if block.tag() != PEM_CERTIFICATE {
                return Err(TlsError::CaMalformed);
            }
            store
                .add(CertificateDer::from(block.into_contents()))
                .map_err(|_| TlsError::CaRejected)?;
        }
        Ok(Self {
            store: Arc::new(store),
        })
    }

    /// Read the CA file at `path` (absolute) through cap-std and parse it. This is blocking file I/O: call it at
    /// startup, before the runtime is busy, or inside `spawn_blocking`.
    ///
    /// The file is opened relative to its directory, and at most [`MAX_CA_FILE_BYTES`] are read.
    pub fn load(path: &Path) -> Result<Self, TlsError> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(TlsError::CaUnreadable);
        };
        let dir = Dir::open_ambient_dir(parent, ambient_authority()).map_err(|_| TlsError::CaUnreadable)?;
        let file = dir.open(name).map_err(|_| TlsError::CaUnreadable)?;
        let limit = u64::try_from(MAX_CA_FILE_BYTES).unwrap_or(u64::MAX);
        let mut text = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut text)
            .map_err(|_| TlsError::CaUnreadable)?;
        if text.len() > MAX_CA_FILE_BYTES {
            return Err(TlsError::CaTooLarge);
        }
        Self::from_pem(&text)
    }

    pub(crate) fn store(&self) -> Arc<RootCertStore> {
        Arc::clone(&self.store)
    }
}

/// The client side of the handshake with the hub. With an `identity` the client certificate is presented, which the
/// `Connect` stream needs; without one nothing is presented, which is how `Join` works when there is no certificate yet.
pub fn client_config(
    roots: &HubRoots,
    identity: Option<&ClientIdentity>,
) -> Result<Arc<ClientConfig>, TlsError> {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let builder = ClientConfig::builder_with_provider(provider)
        // TLS 1.3 and nothing older (S6). Not the library's "safe defaults", which also allow 1.2.
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| TlsError::Config)?
        .with_root_certificates(roots.store());
    let mut config = match identity {
        Some(identity) => builder.with_client_cert_resolver(Arc::new(PresentedIdentity::new(identity)?)),
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = vec![ALPN_H2.to_vec()];
    // A new configuration is built for every connection, so there is nothing to resume from.
    config.resumption = Resumption::disabled();
    Ok(Arc::new(config))
}

/// The agent's certificate chain and its key, for the handshake.
struct PresentedIdentity {
    certified: Arc<CertifiedKey>,
}

impl fmt::Debug for PresentedIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PresentedIdentity([redacted])")
    }
}

impl PresentedIdentity {
    fn new(identity: &ClientIdentity) -> Result<Self, TlsError> {
        let chain = identity
            .chain_der()
            .iter()
            .map(|der| CertificateDer::from(der.to_vec()))
            .collect();
        // The key is borrowed, not copied: rustls loads it into its signing key, and the PKCS#8 bytes stay in
        // `KeyMaterial`, which wipes them on drop.
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key().pkcs8_der()));
        let signing = aws_lc_rs::sign::any_ecdsa_type(&key).map_err(|_| TlsError::ClientKey)?;
        Ok(Self {
            certified: Arc::new(CertifiedKey::new(chain, signing)),
        })
    }
}

impl ResolvesClientCert for PresentedIdentity {
    fn resolve(&self, _hints: &[&[u8]], _schemes: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.certified))
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// What a failed handshake says about the hub. `error` is what `tokio-rustls` returned from `connect`.
pub fn classify(error: &io::Error) -> TlsFailure {
    use rustls::AlertDescription;
    let Some(tls) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    else {
        return TlsFailure::Other;
    };
    match tls {
        rustls::Error::InvalidCertificate(_) => TlsFailure::Certificate,
        rustls::Error::AlertReceived(AlertDescription::ProtocolVersion)
        | rustls::Error::PeerIncompatible(_) => TlsFailure::Version,
        _ => TlsFailure::Other,
    }
}
