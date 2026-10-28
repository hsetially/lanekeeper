//! The web listener's certificate (plan 18, T1, G3; S6, S7): TLS 1.3 only, from the two secrets, replaced without a restart,
//! and unchanged when a replacement is bad. The listener is the hub's own `serve` with the certificate resolver of
//! `process::start`, so this is the code that runs in the process, without a database around it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "support/net.rs"]
mod net;
#[path = "support/secrets.rs"]
mod secrets;
mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use domain::Timestamp;
use hub::config::WebTlsSettings;
use hub::ops::ReadyCheck;
use hub::ready::{CertsReady, Expiring};
use hub::serve::{ServeLimits, serve, tls13_resolver_config};
use hub::web_tls::{PemError, Reloaded, WebCert, WebTlsError};
use hub_platform::clock::{Clock, SystemClock};
use rustls::pki_types::CertificateDer;
use secrets::MemorySecrets;
use support::pki::{Leaf, TestCa};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

struct Rig {
    addr: SocketAddr,
    ca: TestCa,
    cert: Arc<WebCert>,
    secrets: Arc<MemorySecrets>,
    _stop: watch::Sender<bool>,
}

fn settings(reload_every: Duration) -> WebTlsSettings {
    WebTlsSettings {
        cert_ref: "web-cert".to_owned(),
        key_ref: "web-key".to_owned(),
        reload_every,
    }
}

fn publish(secrets: &MemorySecrets, leaf: &Leaf) {
    secrets.put("web-cert", &leaf.cert_pem);
    secrets.put("web-key", &leaf.key_pem);
}

/// A listener on loopback serving `GET /` with the certificate the secrets hold, the way the process serves the web app.
async fn rig(reload_every: Duration, clock: Arc<dyn Clock>) -> (Rig, Leaf) {
    let ca = TestCa::new("web-test-ca");
    let first = ca.leaf(&["localhost"]);
    let secrets = Arc::new(MemorySecrets::default());
    publish(&secrets, &first);
    let cert = WebCert::load(&*secrets, &settings(reload_every), &*clock)
        .await
        .unwrap();
    let acceptor = TlsAcceptor::from(tls13_resolver_config(cert.resolver()).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, mut stopped) = watch::channel(false);
    tokio::spawn(serve(
        listener,
        Some(acceptor),
        Router::new().route("/", get(|| async { "hello" })),
        async move {
            let _ = stopped.wait_for(|s| *s).await;
        },
        ServeLimits::default(),
    ));
    (
        Rig {
            addr,
            ca,
            cert,
            secrets,
            _stop: stop,
        },
        first,
    )
}

/// The first certificate the server presents to a client that trusts `ca`.
async fn presented(addr: SocketAddr, ca: &TestCa) -> CertificateDer<'static> {
    let config = net::client_config(&ca.cert_der(), &[&rustls::version::TLS13]);
    let stream = net::tls_connect(addr, config).await.unwrap();
    stream.get_ref().1.peer_certificates().unwrap()[0]
        .clone()
        .into_owned()
}

#[tokio::test]
async fn the_web_listener_speaks_tls13_only_with_the_loaded_certificate() {
    let (rig, leaf) = rig(Duration::from_secs(300), Arc::new(SystemClock)).await;

    let config = net::client_config(&rig.ca.cert_der(), &[&rustls::version::TLS13]);
    let stream = net::tls_connect(rig.addr, config).await.unwrap();
    let (_, connection) = stream.get_ref();
    assert_eq!(
        connection.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(
        connection.peer_certificates().unwrap()[0],
        leaf.cert,
        "the certificate from the secrets"
    );
    assert_eq!(
        connection.alpn_protocol(),
        Some(&b"http/1.1"[..]),
        "the offered protocol was negotiated"
    );
    let reply = net::exchange(stream, "/").await;
    assert_eq!((reply.status, reply.body.as_str()), (200, "hello"));

    // TLS 1.2 is not spoken at all.
    let old = net::client_config(&rig.ca.cert_der(), &[&rustls::version::TLS12]);
    assert!(
        net::tls_connect(rig.addr, old).await.is_err(),
        "the listener accepted TLS 1.2"
    );

    // And a client that does not trust the certificate's issuer refuses it: the listener presents what it was given, nothing
    // that a stranger's root would accept.
    let stranger = TestCa::new("someone-elses-ca");
    let config = net::client_config(&stranger.cert_der(), &[&rustls::version::TLS13]);
    assert!(net::tls_connect(rig.addr, config).await.is_err());
}

#[tokio::test]
async fn a_replaced_certificate_is_served_to_the_next_handshake() {
    let (rig, first) = rig(Duration::from_secs(300), Arc::new(SystemClock)).await;
    // A connection opened before the replacement keeps working after it, with the certificate it started with.
    let config = net::client_config(&rig.ca.cert_der(), &[&rustls::version::TLS13]);
    let before = net::tls_connect(rig.addr, config).await.unwrap();
    assert_eq!(before.get_ref().1.peer_certificates().unwrap()[0], first.cert);

    let second = rig.ca.leaf(&["localhost"]);
    publish(&rig.secrets, &second);
    assert_eq!(
        rig.cert.reload_once(&*rig.secrets, &SystemClock).await,
        Ok(Reloaded::Replaced)
    );

    assert_eq!(
        presented(rig.addr, &rig.ca).await,
        second.cert,
        "the next handshake gets the new certificate"
    );
    let reply = net::exchange(before, "/").await;
    assert_eq!(reply.status, 200, "the open connection was not cut");
    assert_eq!(rig.cert.metrics().replacements(), 1);
}

#[tokio::test]
async fn an_unusable_replacement_keeps_the_old_certificate() {
    let (rig, first) = rig(Duration::from_secs(300), Arc::new(SystemClock)).await;
    let other = rig.ca.leaf(&["localhost"]);

    // Garbage; a key that is not the certificate's; a secret that vanished.
    rig.secrets.put("web-cert", "not pem");
    assert_eq!(
        rig.cert.reload_once(&*rig.secrets, &SystemClock).await,
        Err(WebTlsError::Pem(PemError::NoCertificate))
    );
    rig.secrets.put("web-cert", &first.cert_pem);
    rig.secrets.put("web-key", &other.key_pem);
    assert_eq!(
        rig.cert.reload_once(&*rig.secrets, &SystemClock).await,
        Err(WebTlsError::Pem(PemError::KeyMismatch))
    );
    rig.secrets.remove("web-key");
    assert!(rig.cert.reload_once(&*rig.secrets, &SystemClock).await.is_err());

    assert_eq!(
        presented(rig.addr, &rig.ca).await,
        first.cert,
        "still the first certificate"
    );
    assert_eq!(rig.cert.metrics().failures(), 3, "each refusal is counted");
    assert_eq!(rig.cert.metrics().replacements(), 0);
}

#[derive(Debug)]
struct Moving(AtomicI64);

impl Clock for Moving {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.0.load(Ordering::SeqCst))
    }
}

#[tokio::test]
async fn an_expired_certificate_fails_readiness_and_is_never_replaced_by_an_expired_one() {
    let (rig, first) = rig(Duration::from_secs(300), Arc::new(SystemClock)).await;
    let expiry = rig.cert.not_after();
    let clock = Arc::new(Moving(AtomicI64::new(expiry.unix_millis() - 60_000)));
    let ready = CertsReady::new(clock.clone(), vec![rig.cert.clone() as Arc<dyn Expiring>]);
    assert!(ready.ready().await, "a minute before it expires");
    clock.0.store(expiry.unix_millis() + 1, Ordering::SeqCst);
    assert!(!ready.ready().await, "after it expires");
    // The listener goes on serving what it has: failing readiness is how the hub asks a person to act, not a crash.
    assert_eq!(presented(rig.addr, &rig.ca).await, first.cert);
    // A replacement that is itself already expired (by that clock) is refused too.
    assert_eq!(
        rig.cert.reload_once(&*rig.secrets, &*clock).await,
        Err(WebTlsError::Expired)
    );
}

#[tokio::test]
async fn the_timer_picks_up_a_rotated_certificate() {
    let (rig, first) = rig(Duration::from_secs(1), Arc::new(SystemClock)).await;
    let (close, closing) = watch::channel(false);
    let task = Arc::clone(&rig.cert).spawn_reload(rig.secrets.clone(), Arc::new(SystemClock), closing);
    let second = rig.ca.leaf(&["localhost"]);
    publish(&rig.secrets, &second);
    // Every wait is the interval plus or minus a fifth, so within two seconds there has been at least one read.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let now = presented(rig.addr, &rig.ca).await;
        if now == second.cert {
            break;
        }
        assert_eq!(now, first.cert);
        assert!(
            Instant::now() < deadline,
            "the rotated certificate was never served"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    close.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

/// S7: the certificate and the key reach the process by one road. The settings hold two secret names; the module reads them
/// through the `SecretSource` port; no file, no environment variable and no Kubernetes Secret carries either.
#[test]
fn certificate_and_key_come_from_the_secrets_source_only() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let code = |name: &str| {
        let text = std::fs::read_to_string(root.join(name)).unwrap();
        text.split("#[cfg(test)]").next().unwrap().to_owned()
    };
    let module = code("src/web_tls.rs");
    for forbidden in [
        "std::fs",
        "std::env",
        "tokio::fs",
        "File::open",
        "read_to_string",
        "var_os",
    ] {
        assert!(
            !module.contains(forbidden),
            "web_tls.rs reads outside the secrets source: {forbidden}"
        );
    }
    let group = code("src/config/web_tls.rs");
    for forbidden in ["_FILE", "_PEM", "std::fs", "std::env"] {
        assert!(
            !group.contains(forbidden),
            "the web TLS settings name a file or hold PEM: {forbidden}"
        );
    }
    // The pod contract passes names, never a certificate or a key.
    let contract = std::fs::read_to_string(root.join("HARDENING.md")).unwrap();
    for forbidden in [
        "LK_WEB_CERT_FILE",
        "LK_WEB_KEY_FILE",
        "LK_WEB_CERT_PEM",
        "tls.crt",
        "kubernetes.io/tls",
    ] {
        assert!(
            !contract.contains(forbidden),
            "HARDENING.md carries the web certificate by another road: {forbidden}"
        );
    }
    let settings = HubSettingsProbe::read();
    assert_eq!(settings, ("web-cert".to_owned(), "web-key".to_owned()));
}

/// The two settings, read the way `main` reads them.
struct HubSettingsProbe;

impl HubSettingsProbe {
    fn read() -> (String, String) {
        let tls = hub::HubSettings::from_lookup(|name| match name {
            "LK_WEB_CERT_REF" => Some("web-cert".to_owned()),
            "LK_WEB_KEY_REF" => Some("web-key".to_owned()),
            _ => None,
        })
        .unwrap()
        .web_tls
        .unwrap();
        (tls.cert_ref, tls.key_ref)
    }
}
