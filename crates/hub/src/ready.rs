//! What `/readyz` checks beyond the database (plan 18, G11): certificates, the event bus and the job runner.
//!
//! "Both CA probes passed" is a fact about the start, so as a check it would be constant; what can change while the process
//! runs is that a certificate it holds runs out because its renewal keeps failing, that the bus's `LISTEN` connection drops,
//! or that the job runner stops. Those three are the checks. Each answers from memory: the probe gives all of them together
//! two seconds (`ops::READY_DEADLINE`), so none of them makes a call.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Timestamp;
use hub_platform::clock::Clock;
use hub_platform::tls::RotatingServerCert;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::ops::ReadyCheck;

/// A certificate the process holds and serves with, and the time it stops being valid.
pub trait Expiring: Send + Sync + 'static {
    fn not_after(&self) -> Timestamp;
}

/// A server certificate that is replaced as it is renewed (the agent and replica listeners'): the `notAfter` of whatever it holds now.
///
/// Read from the certificate on each ask, so a renewal is seen the moment it is swapped in. A certificate that cannot be read
/// counts as expired, which fails readiness: the safe way to be wrong.
#[derive(Debug)]
pub struct RotatingExpiry(Arc<RotatingServerCert>);

impl RotatingExpiry {
    #[must_use]
    pub fn new(cell: Arc<RotatingServerCert>) -> Self {
        Self(cell)
    }
}

impl Expiring for RotatingExpiry {
    fn not_after(&self) -> Timestamp {
        let held = self.0.current();
        let Some(leaf) = held.cert.first() else {
            return Timestamp::from_unix_millis(0);
        };
        match X509Certificate::from_der(leaf.as_ref()) {
            Ok((_, cert)) => {
                Timestamp::from_unix_millis(cert.validity().not_after.timestamp().saturating_mul(1000))
            }
            Err(_) => Timestamp::from_unix_millis(0),
        }
    }
}

/// Ready while every certificate held is still valid. A renewal that keeps failing shows up here when the old certificate
/// runs out, which is when a person must act; one failed attempt does not, because the old certificate still works.
pub struct CertsReady {
    clock: Arc<dyn Clock>,
    held: Vec<Arc<dyn Expiring>>,
}

impl fmt::Debug for CertsReady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertsReady")
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

impl CertsReady {
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, held: Vec<Arc<dyn Expiring>>) -> Self {
        Self { clock, held }
    }
}

#[async_trait]
impl ReadyCheck for CertsReady {
    async fn ready(&self) -> bool {
        let now = self.clock.now();
        self.held.iter().all(|cert| cert.not_after() > now)
    }
}

/// A condition that is a bool read from memory: "the bus has its `LISTEN` connection", "the job runner is running".
#[derive(Clone)]
pub struct FlagReady(Arc<dyn Fn() -> bool + Send + Sync>);

impl fmt::Debug for FlagReady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FlagReady")
    }
}

impl FlagReady {
    #[must_use]
    pub fn new(flag: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(flag))
    }
}

#[async_trait]
impl ReadyCheck for FlagReady {
    async fn ready(&self) -> bool {
        (self.0)()
    }
}

/// Ready while the event bus holds its `LISTEN` connection (`PgEventBus::listener_connected`, wired in T3).
pub type EventsReady = FlagReady;

/// Ready while the job runner is running (wired in T3).
pub type JobsReady = FlagReady;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

    use super::*;

    #[derive(Debug)]
    struct Now(AtomicI64);

    impl Clock for Now {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0.load(Ordering::SeqCst))
        }
    }

    struct Until(i64);

    impl Expiring for Until {
        fn not_after(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0)
        }
    }

    #[tokio::test]
    async fn certificates_are_ready_until_the_first_of_them_expires() {
        let clock = Arc::new(Now(AtomicI64::new(1_000)));
        let check = CertsReady::new(
            clock.clone(),
            vec![Arc::new(Until(5_000)), Arc::new(Until(3_000))],
        );
        assert!(check.ready().await);
        clock.0.store(2_999, Ordering::SeqCst);
        assert!(check.ready().await, "one millisecond left is still valid");
        clock.0.store(3_000, Ordering::SeqCst);
        assert!(
            !check.ready().await,
            "expired at the instant it stops being valid"
        );
        clock.0.store(4_000, Ordering::SeqCst);
        assert!(!check.ready().await, "one expired certificate is enough");
    }

    #[tokio::test]
    async fn holding_no_certificate_is_ready() {
        let check = CertsReady::new(Arc::new(Now(AtomicI64::new(0))), Vec::new());
        assert!(check.ready().await);
    }

    #[tokio::test]
    async fn a_flag_follows_what_it_reads() {
        let up = Arc::new(AtomicBool::new(false));
        let reader = Arc::clone(&up);
        let check: EventsReady = FlagReady::new(move || reader.load(Ordering::SeqCst));
        assert!(!check.ready().await);
        up.store(true, Ordering::SeqCst);
        assert!(check.ready().await);
        let jobs: JobsReady = check.clone();
        up.store(false, Ordering::SeqCst);
        assert!(!jobs.ready().await);
    }
}
