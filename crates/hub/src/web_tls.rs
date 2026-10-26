//! The web listener's certificate (plan 18, T1, G3, S6, S7): read from two Secret Manager secrets at start, read again on a
//! timer, and swapped in without a restart.
//!
//! - **Where it comes from.** The settings name two secrets: the chain (leaf first, PEM) and the key (PEM). Nothing else
//!   carries a certificate into the process: not a variable, not a Kubernetes Secret, not a file the pod mounts.
//! - **PEM is a trust boundary.** The text comes from a secret someone else writes, so [`decode`] bounds it (size, number of
//!   certificates), reads only what it understands, checks that the key belongs to the leaf, and returns errors that are fixed
//!   words. It never repeats the text. A fuzz target (`hub_pem`) runs it on arbitrary bytes.
//! - **A bad replacement changes nothing.** [`WebCert::reload_once`] builds the new certificate completely before it touches the
//!   one in use. If anything is wrong (a secret missing, a key that does not match, a certificate already expired) the old
//!   certificate stays, the failure is counted and logged by the secret's name, and the next tick tries again. Two secrets
//!   cannot be rotated in one step, so a tick can see a new chain with the old key; that is the same case and heals itself.
//! - **Expiry is a readiness condition, not an exit.** [`WebCert`] says when the held leaf stops being valid
//!   ([`Expiring`]); `/readyz` fails then (`ready::CertsReady`), which is when a person must act.
//! - **New connections get the new certificate.** The listener resolves the certificate at each handshake
//!   ([`RotatingServerCert`]); connections already open keep the one they started with.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use domain::{Secret, Timestamp};
use hub_platform::clock::Clock;
use hub_platform::tls::RotatingServerCert;
use ports::{SecretError, SecretSource};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use rustls::{Error as RustlsError, InconsistentKeys};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use x509_parser::prelude::{FromDer, X509Certificate};
use zeroize::Zeroize;

use crate::config::WebTlsSettings;
use crate::ready::Expiring;

/// The largest chain secret read. A chain of a few certificates is a few KiB.
pub const MAX_CHAIN_PEM_BYTES: usize = 64 * 1024;
/// The largest key secret read. A 4096-bit RSA key in PEM is about 3 KiB.
pub const MAX_KEY_PEM_BYTES: usize = 16 * 1024;
/// The most certificates a chain may hold: a leaf, its intermediates, and room for a cross-signed root.
pub const MAX_CHAIN_LEN: usize = 8;
/// The longest one reload may take (two secret reads and a decode). The Google client has its own deadline; this is the cap
/// on top, so a stuck call cannot stop the timer for good.
pub const RELOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Why PEM text was not turned into a certificate. Every message is fixed: none carries the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PemError {
    #[error("the certificate chain is too large")]
    ChainTooLarge,
    #[error("the private key is too large")]
    KeyTooLarge,
    #[error("the certificate chain holds no certificate")]
    NoCertificate,
    #[error("the certificate chain holds too many certificates")]
    TooManyCertificates,
    #[error("the certificate chain cannot be read")]
    ChainUnreadable,
    #[error("the leaf certificate cannot be read")]
    LeafUnreadable,
    #[error("the private key cannot be read")]
    KeyUnreadable,
    #[error("the private key does not belong to the leaf certificate")]
    KeyMismatch,
    #[error("the private key is not one the listener can use")]
    KeyUnusable,
}

/// A certificate that decoded, with the times the leaf is valid.
pub struct Decoded {
    pub key: Arc<CertifiedKey>,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

impl fmt::Debug for Decoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The key is a secret; the times are not.
        f.debug_struct("Decoded")
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .finish_non_exhaustive()
    }
}

/// Turn a chain and a key, both PEM, into something the listener can serve with.
///
/// Other PEM sections in either text (a comment block, a second key) are skipped, not trusted. The copy of the key made for
/// rustls is wiped when it has been loaded; rustls takes ownership of its own copy, which is freed without wiping (a residual,
/// recorded in the threat model).
pub fn decode(chain_pem: &Secret<String>, key_pem: &Secret<String>) -> Result<Decoded, PemError> {
    let (chain_text, key_text) = (chain_pem.expose().as_bytes(), key_pem.expose().as_bytes());
    if chain_text.len() > MAX_CHAIN_PEM_BYTES {
        return Err(PemError::ChainTooLarge);
    }
    if key_text.len() > MAX_KEY_PEM_BYTES {
        return Err(PemError::KeyTooLarge);
    }
    let mut chain = Vec::new();
    for item in CertificateDer::pem_slice_iter(chain_text) {
        chain.push(item.map_err(|_| PemError::ChainUnreadable)?);
        if chain.len() > MAX_CHAIN_LEN {
            return Err(PemError::TooManyCertificates);
        }
    }
    let leaf = chain.first().ok_or(PemError::NoCertificate)?;
    let (not_before, not_after) = {
        let (rest, cert) = X509Certificate::from_der(leaf.as_ref()).map_err(|_| PemError::LeafUnreadable)?;
        if !rest.is_empty() {
            return Err(PemError::LeafUnreadable);
        }
        let validity = cert.validity();
        (
            Timestamp::from_unix_millis(validity.not_before.timestamp().saturating_mul(1000)),
            Timestamp::from_unix_millis(validity.not_after.timestamp().saturating_mul(1000)),
        )
    };
    let mut key = PrivateKeyDer::from_pem_slice(key_text).map_err(|_| PemError::KeyUnreadable)?;
    let provider = aws_lc_rs::default_provider();
    let loaded = CertifiedKey::from_der(chain, key.clone_key(), &provider);
    key.zeroize();
    match loaded {
        Ok(certified) => Ok(Decoded {
            key: Arc::new(certified),
            not_before,
            not_after,
        }),
        Err(RustlsError::InconsistentKeys(InconsistentKeys::KeyMismatch)) => Err(PemError::KeyMismatch),
        Err(_) => Err(PemError::KeyUnusable),
    }
}

/// Why the web certificate could not be loaded or replaced. Fixed words; the secret's name is logged next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WebTlsError {
    #[error("the certificate secret could not be read")]
    ChainSecret(SecretError),
    #[error("the key secret could not be read")]
    KeySecret(SecretError),
    #[error(transparent)]
    Pem(#[from] PemError),
    #[error("the certificate has already expired")]
    Expired,
    #[error("reading the certificate took too long")]
    TimedOut,
}

/// What reloading found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reloaded {
    /// The secrets hold the certificate that is already in use.
    Unchanged,
    /// A new certificate is in use from the next handshake on.
    Replaced,
}

/// The counters of the certificate's life, for `/metrics`.
#[derive(Debug, Clone, Default)]
pub struct WebCertMetrics {
    replaced: Counter,
    failed: Counter,
    not_after_seconds: Gauge,
}

impl WebCertMetrics {
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "web_cert_replaced",
            "Times the web certificate was replaced by a newer one",
            self.replaced.clone(),
        );
        registry.register(
            "web_cert_reload_failures",
            "Times reading a replacement for the web certificate failed; the old one stayed in use",
            self.failed.clone(),
        );
        registry.register(
            "web_cert_not_after_seconds",
            "When the web certificate in use stops being valid, in seconds since the Unix epoch",
            self.not_after_seconds.clone(),
        );
    }

    /// Times a bad replacement was refused.
    #[must_use]
    pub fn failures(&self) -> u64 {
        self.failed.get()
    }

    /// Times a replacement was put in use.
    #[must_use]
    pub fn replacements(&self) -> u64 {
        self.replaced.get()
    }
}

/// The certificate the web listener serves, and the means to keep it fresh.
pub struct WebCert {
    cert_ref: String,
    key_ref: String,
    reload_every: Duration,
    resolver: Arc<RotatingServerCert>,
    not_after_ms: AtomicI64,
    metrics: WebCertMetrics,
}

impl fmt::Debug for WebCert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebCert")
            .field("not_after_ms", &self.not_after_ms.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

async fn read(
    secrets: &dyn SecretSource,
    cert_ref: &str,
    key_ref: &str,
    clock: &dyn Clock,
) -> Result<Decoded, WebTlsError> {
    let (chain, key) = tokio::try_join!(
        async { secrets.get(cert_ref).await.map_err(WebTlsError::ChainSecret) },
        async { secrets.get(key_ref).await.map_err(WebTlsError::KeySecret) },
    )?;
    let decoded = decode(&chain, &key)?;
    if decoded.not_after <= clock.now() {
        return Err(WebTlsError::Expired);
    }
    Ok(decoded)
}

impl WebCert {
    /// Read the certificate for the first time. Any problem is an error: a hub with no certificate has nothing to serve with.
    pub async fn load(
        secrets: &dyn SecretSource,
        settings: &WebTlsSettings,
        clock: &dyn Clock,
    ) -> Result<Arc<Self>, WebTlsError> {
        let decoded = read(secrets, &settings.cert_ref, &settings.key_ref, clock).await?;
        let metrics = WebCertMetrics::default();
        metrics
            .not_after_seconds
            .set(decoded.not_after.unix_millis() / 1000);
        Ok(Arc::new(Self {
            cert_ref: settings.cert_ref.clone(),
            key_ref: settings.key_ref.clone(),
            reload_every: settings.reload_every,
            not_after_ms: AtomicI64::new(decoded.not_after.unix_millis()),
            resolver: RotatingServerCert::new(decoded.key),
            metrics,
        }))
    }

    /// What the listener's TLS configuration asks for a certificate at each handshake.
    #[must_use]
    pub fn resolver(&self) -> Arc<RotatingServerCert> {
        Arc::clone(&self.resolver)
    }

    #[must_use]
    pub fn metrics(&self) -> &WebCertMetrics {
        &self.metrics
    }

    /// Read both secrets and, if they hold a different and valid certificate, use it from the next handshake on. On any error
    /// nothing changes and the failure is counted.
    pub async fn reload_once(
        &self,
        secrets: &dyn SecretSource,
        clock: &dyn Clock,
    ) -> Result<Reloaded, WebTlsError> {
        let outcome = match read(secrets, &self.cert_ref, &self.key_ref, clock).await {
            Ok(decoded) => {
                let same = self.resolver.current().cert.first() == decoded.key.cert.first();
                if same {
                    Ok(Reloaded::Unchanged)
                } else {
                    self.not_after_ms
                        .store(decoded.not_after.unix_millis(), Ordering::SeqCst);
                    self.metrics
                        .not_after_seconds
                        .set(decoded.not_after.unix_millis() / 1000);
                    self.resolver.replace(decoded.key);
                    self.metrics.replaced.inc();
                    Ok(Reloaded::Replaced)
                }
            }
            Err(error) => Err(error),
        };
        if outcome.is_err() {
            self.metrics.failed.inc();
        }
        outcome
    }

    /// Reload on a timer until `closing` turns true. Each wait is the setting's interval, plus or minus a fifth at random, so
    /// replicas do not all read in the same second.
    pub fn spawn_reload(
        self: Arc<Self>,
        secrets: Arc<dyn SecretSource>,
        clock: Arc<dyn Clock>,
        mut closing: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Also ends if the sender is gone: nobody is left to say stop, and the process is ending.
                    _ = closing.wait_for(|c| *c) => return,
                    () = tokio::time::sleep(jittered(self.reload_every)) => {}
                }
                let attempt = tokio::time::timeout(RELOAD_TIMEOUT, self.reload_once(&*secrets, &*clock))
                    .await
                    .unwrap_or_else(|_| {
                        self.metrics.failed.inc();
                        Err(WebTlsError::TimedOut)
                    });
                match attempt {
                    Ok(Reloaded::Replaced) => {
                        info!(cert = %self.cert_ref, "the web certificate was replaced");
                    }
                    Ok(Reloaded::Unchanged) => debug!("the web certificate is unchanged"),
                    Err(error) => warn!(
                        cert = %self.cert_ref,
                        key = %self.key_ref,
                        %error,
                        "the web certificate was not replaced; the one in use stays"
                    ),
                }
            }
        })
    }
}

impl Expiring for WebCert {
    fn not_after(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.not_after_ms.load(Ordering::SeqCst))
    }
}

/// `base` plus or minus a fifth, at random.
#[must_use]
pub fn jittered(base: Duration) -> Duration {
    base.mul_f64(0.8 + 0.4 * fastrand::f64())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::PoisonError;

    use async_trait::async_trait;
    use rcgen::{CertificateParams, KeyPair, date_time_ymd};

    use super::*;

    /// A chain and key as PEM, valid between the two years given.
    struct Made {
        chain: String,
        key: String,
    }

    fn make(name: &str, from_year: i32, to_year: i32) -> Made {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec![name.to_owned()]).unwrap();
        params.not_before = date_time_ymd(from_year, 1, 1);
        params.not_after = date_time_ymd(to_year, 12, 31);
        let cert = params.self_signed(&key).unwrap();
        Made {
            chain: cert.pem(),
            key: key.serialize_pem(),
        }
    }

    fn secret(text: &str) -> Secret<String> {
        Secret::new(text.to_owned())
    }

    /// Secrets that a test can change while the hub runs.
    #[derive(Default)]
    struct Swappable(Mutex<HashMap<String, String>>);

    impl Swappable {
        fn put(&self, name: &str, value: &str) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(name.to_owned(), value.to_owned());
        }

        fn put_cert(&self, made: &Made) {
            self.put("web-cert", &made.chain);
            self.put("web-key", &made.key);
        }
    }

    #[async_trait]
    impl SecretSource for Swappable {
        async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(name)
                .map(|v| Secret::new(v.clone()))
                .ok_or(SecretError::NotFound)
        }
    }

    #[derive(Debug)]
    struct Now(AtomicI64);

    impl Clock for Now {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0.load(Ordering::SeqCst))
        }
    }

    /// A clock in 2025, between the certificates made below.
    fn clock() -> Arc<Now> {
        Arc::new(Now(AtomicI64::new(
            date_time_ymd(2025, 6, 1).unix_timestamp() * 1000,
        )))
    }

    fn settings(reload_every: Duration) -> WebTlsSettings {
        WebTlsSettings {
            cert_ref: "web-cert".to_owned(),
            key_ref: "web-key".to_owned(),
            reload_every,
        }
    }

    #[test]
    fn a_chain_and_its_key_decode_and_the_expiry_is_read_from_the_leaf() {
        let made = make("hub.example", 2020, 2099);
        let decoded = decode(&secret(&made.chain), &secret(&made.key)).unwrap();
        assert_eq!(
            decoded.not_after.unix_millis(),
            date_time_ymd(2099, 12, 31).unix_timestamp() * 1000
        );
        assert_eq!(
            decoded.not_before.unix_millis(),
            date_time_ymd(2020, 1, 1).unix_timestamp() * 1000
        );
        assert_eq!(decoded.key.cert.len(), 1);
    }

    #[test]
    fn a_chain_of_several_certificates_keeps_the_leaf_first() {
        let leaf = make("hub.example", 2020, 2099);
        let other = make("ca.example", 2020, 2099);
        let chain = format!("{}\n{}", leaf.chain, other.chain);
        let decoded = decode(&secret(&chain), &secret(&leaf.key)).unwrap();
        assert_eq!(decoded.key.cert.len(), 2);
    }

    #[test]
    fn sections_that_are_not_certificates_are_skipped_not_trusted() {
        let made = make("hub.example", 2020, 2099);
        let other = make("other.example", 2020, 2099);
        // A stray key block in the chain secret, and a stray certificate in the key secret: neither is used.
        let chain = format!("{}\n{}", other.key, made.chain);
        let key = format!("{}\n{}", other.chain, made.key);
        assert!(decode(&secret(&chain), &secret(&key)).is_ok());
    }

    #[test]
    fn a_key_that_belongs_to_another_certificate_is_refused() {
        let (a, b) = (make("a.example", 2020, 2099), make("b.example", 2020, 2099));
        assert_eq!(
            decode(&secret(&a.chain), &secret(&b.key)).unwrap_err(),
            PemError::KeyMismatch
        );
    }

    #[test]
    fn nothing_that_is_not_a_chain_and_a_key_decodes() {
        let made = make("hub.example", 2020, 2099);
        let cases: Vec<(String, String, PemError)> = vec![
            (String::new(), made.key.clone(), PemError::NoCertificate),
            (
                "not pem at all".to_owned(),
                made.key.clone(),
                PemError::NoCertificate,
            ),
            (made.key.clone(), made.key.clone(), PemError::NoCertificate),
            (made.chain.clone(), String::new(), PemError::KeyUnreadable),
            (made.chain.clone(), made.chain.clone(), PemError::KeyUnreadable),
            (
                "-----BEGIN CERTIFICATE-----\n!!!notbase64!!!\n-----END CERTIFICATE-----\n".to_owned(),
                made.key.clone(),
                PemError::ChainUnreadable,
            ),
            (
                // Valid base64, valid PEM framing, but not a certificate.
                "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_owned(),
                made.key.clone(),
                PemError::LeafUnreadable,
            ),
            (
                made.chain.repeat(MAX_CHAIN_LEN + 1),
                made.key.clone(),
                PemError::TooManyCertificates,
            ),
            (
                " ".repeat(MAX_CHAIN_PEM_BYTES + 1),
                made.key.clone(),
                PemError::ChainTooLarge,
            ),
            (
                made.chain.clone(),
                " ".repeat(MAX_KEY_PEM_BYTES + 1),
                PemError::KeyTooLarge,
            ),
        ];
        for (chain, key, want) in cases {
            assert_eq!(decode(&secret(&chain), &secret(&key)).unwrap_err(), want);
        }
    }

    #[test]
    fn an_error_never_contains_the_text_it_was_given() {
        let marker = "zz9q-planted-secret-text";
        for (chain, key) in [
            (marker.to_owned(), marker.to_owned()),
            (
                format!("-----BEGIN CERTIFICATE-----\n{marker}\n-----END CERTIFICATE-----\n"),
                marker.to_owned(),
            ),
            (
                make("hub.example", 2020, 2099).chain,
                format!("-----BEGIN PRIVATE KEY-----\n{marker}\n-----END PRIVATE KEY-----\n"),
            ),
        ] {
            let error = decode(&secret(&chain), &secret(&key)).unwrap_err();
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains("zz9q"), "{shown}");
        }
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_text_never_panics(chain in ".{0,300}", key in ".{0,300}") {
            let _ = decode(&secret(&chain), &secret(&key));
        }

        #[test]
        fn the_jitter_stays_within_a_fifth(secs in 1u64..86_400) {
            let base = Duration::from_secs(secs);
            let wait = jittered(base);
            proptest::prop_assert!(wait >= base.mul_f64(0.8) && wait <= base.mul_f64(1.2));
        }
    }

    #[tokio::test]
    async fn the_certificate_is_loaded_from_the_two_named_secrets() {
        let secrets = Swappable::default();
        let made = make("hub.example", 2020, 2099);
        secrets.put_cert(&made);
        let clock = clock();
        let cert = WebCert::load(&secrets, &settings(Duration::from_secs(300)), &*clock)
            .await
            .unwrap();
        assert_eq!(
            cert.not_after().unix_millis(),
            date_time_ymd(2099, 12, 31).unix_timestamp() * 1000
        );
        assert_eq!(cert.resolver().current().cert.len(), 1);
    }

    #[tokio::test]
    async fn a_missing_secret_a_bad_pem_and_an_expired_certificate_all_refuse_to_load() {
        let clock = clock();
        let s = settings(Duration::from_secs(300));
        let secrets = Swappable::default();
        assert_eq!(
            WebCert::load(&secrets, &s, &*clock).await.unwrap_err(),
            WebTlsError::ChainSecret(SecretError::NotFound)
        );
        secrets.put("web-cert", &make("hub.example", 2020, 2099).chain);
        assert_eq!(
            WebCert::load(&secrets, &s, &*clock).await.unwrap_err(),
            WebTlsError::KeySecret(SecretError::NotFound)
        );
        secrets.put("web-key", "garbage");
        assert_eq!(
            WebCert::load(&secrets, &s, &*clock).await.unwrap_err(),
            WebTlsError::Pem(PemError::KeyUnreadable)
        );
        // Valid in the past only: the clock says 2025.
        secrets.put_cert(&make("hub.example", 2019, 2021));
        assert_eq!(
            WebCert::load(&secrets, &s, &*clock).await.unwrap_err(),
            WebTlsError::Expired
        );
    }

    #[tokio::test]
    async fn a_good_replacement_is_used_and_an_unchanged_one_is_not_counted() {
        let secrets = Swappable::default();
        let first = make("one.example", 2020, 2098);
        secrets.put_cert(&first);
        let clock = clock();
        let cert = WebCert::load(&secrets, &settings(Duration::from_secs(300)), &*clock)
            .await
            .unwrap();
        let before = cert.resolver().current().cert[0].clone();
        assert_eq!(cert.reload_once(&secrets, &*clock).await, Ok(Reloaded::Unchanged));
        assert_eq!(cert.metrics().replacements(), 0);

        let second = make("two.example", 2020, 2099);
        secrets.put_cert(&second);
        assert_eq!(cert.reload_once(&secrets, &*clock).await, Ok(Reloaded::Replaced));
        assert_ne!(cert.resolver().current().cert[0], before);
        assert_eq!(cert.metrics().replacements(), 1);
        assert_eq!(
            cert.not_after().unix_millis(),
            date_time_ymd(2099, 12, 31).unix_timestamp() * 1000,
            "the expiry follows the certificate in use"
        );
    }

    #[tokio::test]
    async fn an_unusable_replacement_keeps_the_old_certificate_and_is_counted() {
        let secrets = Swappable::default();
        secrets.put_cert(&make("one.example", 2020, 2098));
        let clock = clock();
        let cert = WebCert::load(&secrets, &settings(Duration::from_secs(300)), &*clock)
            .await
            .unwrap();
        let before = cert.resolver().current().cert[0].clone();
        let expiry = cert.not_after();

        // Garbage; a chain with another certificate's key; an already expired certificate; a secret that went missing.
        let other = make("other.example", 2020, 2099);
        let bad: Vec<(String, String, WebTlsError)> = vec![
            (
                "junk".to_owned(),
                "junk".to_owned(),
                WebTlsError::Pem(PemError::NoCertificate),
            ),
            (
                other.chain.clone(),
                make("x.example", 2020, 2099).key,
                WebTlsError::Pem(PemError::KeyMismatch),
            ),
            {
                let old = make("old.example", 2019, 2021);
                (old.chain, old.key, WebTlsError::Expired)
            },
        ];
        for (n, (chain, key, want)) in bad.into_iter().enumerate() {
            secrets.put("web-cert", &chain);
            secrets.put("web-key", &key);
            assert_eq!(cert.reload_once(&secrets, &*clock).await, Err(want));
            assert_eq!(cert.metrics().failures(), n as u64 + 1);
            assert_eq!(
                cert.resolver().current().cert[0],
                before,
                "the old certificate is still served"
            );
            assert_eq!(cert.not_after(), expiry);
        }
        secrets.0.lock().unwrap().remove("web-key");
        assert_eq!(
            cert.reload_once(&secrets, &*clock).await,
            Err(WebTlsError::KeySecret(SecretError::NotFound))
        );
        assert_eq!(cert.resolver().current().cert[0], before);
        assert_eq!(cert.metrics().replacements(), 0);
    }

    /// Let every task that can run, run, without moving the clock.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_timer_replaces_the_certificate_and_a_failing_tick_changes_nothing() {
        let secrets = Arc::new(Swappable::default());
        secrets.put_cert(&make("one.example", 2020, 2098));
        let clock = clock();
        let cert = WebCert::load(&*secrets, &settings(Duration::from_secs(300)), &*clock)
            .await
            .unwrap();
        let first = cert.resolver().current().cert[0].clone();
        let (close, closing) = watch::channel(false);
        let task = Arc::clone(&cert).spawn_reload(secrets.clone(), clock.clone(), closing);

        // The task must have started and registered its sleep before the clock moves.
        settle().await;

        // A tick with garbage in the secrets: still the first certificate, one failure counted.
        secrets.put("web-cert", "junk");
        tokio::time::advance(Duration::from_secs(361)).await;
        settle().await;
        assert_eq!(cert.metrics().failures(), 1);
        assert_eq!(cert.resolver().current().cert[0], first);

        // The next tick finds a good one.
        let second = make("two.example", 2020, 2099);
        secrets.put_cert(&second);
        tokio::time::advance(Duration::from_secs(361)).await;
        settle().await;
        assert_eq!(cert.metrics().replacements(), 1);
        assert_ne!(cert.resolver().current().cert[0], first);

        // Closing ends the task.
        close.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn the_timer_does_not_fire_early() {
        let secrets = Arc::new(Swappable::default());
        secrets.put_cert(&make("one.example", 2020, 2098));
        let clock = clock();
        let cert = WebCert::load(&*secrets, &settings(Duration::from_secs(300)), &*clock)
            .await
            .unwrap();
        let (_close, closing) = watch::channel(false);
        let _task = Arc::clone(&cert).spawn_reload(secrets.clone(), clock.clone(), closing);
        settle().await;
        secrets.put("web-cert", "junk");
        // The shortest wait is 240 seconds (a fifth under 300).
        tokio::time::advance(Duration::from_secs(239)).await;
        settle().await;
        assert_eq!(
            cert.metrics().failures(),
            0,
            "no read before the jittered interval"
        );
        // And it does read once the longest wait (360 seconds) is over: the test above is not passing because nothing runs.
        tokio::time::advance(Duration::from_secs(122)).await;
        settle().await;
        assert_eq!(cert.metrics().failures(), 1);
    }

    #[test]
    fn the_metrics_expose_the_expiry_and_the_counters() {
        let metrics = WebCertMetrics::default();
        metrics.failed.inc();
        metrics.not_after_seconds.set(4_000_000_000);
        let mut registry = Registry::with_prefix("lanekeeper");
        metrics.register(&mut registry);
        let mut out = String::new();
        prometheus_client::encoding::text::encode(&mut out, &registry).unwrap();
        assert!(
            out.contains("lanekeeper_web_cert_reload_failures_total 1"),
            "{out}"
        );
        assert!(
            out.contains("lanekeeper_web_cert_not_after_seconds 4000000000"),
            "{out}"
        );
        assert!(out.contains("lanekeeper_web_cert_replaced_total 0"), "{out}");
    }
}
