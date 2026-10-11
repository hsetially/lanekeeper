//! The audit component wired to a real Postgres: the chain, a real P-256 signer, an in-memory bucket, fake leases and a fake
//! notifier. The helpers at the bottom do what only a database owner can do, to tamper with history.

use std::sync::Arc;

use domain::Timestamp;
use hub_identity::audit::{
    ChainVerifier, CheckpointDeps, CheckpointJob, EcdsaP256Verifier, MemoryBucket, NightlyVerifyJob,
    PgAuditLog,
};
use hub_identity::metrics::IdentityMetrics;
use hub_identity::{Clock, ManualClock};
use ports::conformance::sample::user_id;
use ports::fakes::{FakeLeases, FakeNotifier};
use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, Tx};
use sqlx::PgPool;

use super::ecdsa_kms::EcdsaKms;
use super::pg::TestDb;

pub const VERSION: &str = "projects/lk/locations/eu/keyRings/audit/cryptoKeys/checkpoint/cryptoKeyVersions/1";

pub struct AuditRig {
    pub db: TestDb,
    pub pool: PgPool,
    pub leases: Arc<FakeLeases>,
    pub kms: Arc<EcdsaKms>,
    pub bucket: Arc<MemoryBucket>,
    pub notifier: FakeNotifier,
    pub metrics: Arc<IdentityMetrics>,
    pub clock: Arc<ManualClock>,
}

pub fn event(n: i64) -> AuditEvent {
    AuditEvent::new(
        Timestamp::from_unix_millis(1_791_676_800_000 + n),
        AuditActor::User { user: user_id(1) },
        AuditAction::SettingsChanged,
        AuditVia::Ui,
    )
}

impl AuditRig {
    pub async fn new(db: TestDb) -> Self {
        let pool = db.app_pool(8).await;
        let kms = Arc::new(EcdsaKms::new(pool.clone()));
        Self {
            db,
            pool,
            leases: Arc::new(FakeLeases::new()),
            kms,
            bucket: Arc::new(MemoryBucket::new()),
            notifier: FakeNotifier::new(),
            metrics: Arc::new(IdentityMetrics::new()),
            clock: Arc::new(ManualClock::at(Timestamp::from_unix_millis(1_791_676_900_000))),
        }
    }

    /// Commit `n` more events, one transaction each. Returns the `seq` of the last.
    pub async fn record(&self, n: usize) -> i64 {
        let start: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
            .fetch_one(&self.pool)
            .await
            .unwrap();
        for i in 0..n {
            let mut tx = self.pool.begin().await.unwrap();
            PgAuditLog::new()
                .record(
                    &mut Tx::from_pg(&mut tx),
                    event(start + i64::try_from(i).unwrap()),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        sqlx::query_scalar("SELECT max(seq) FROM audit_events")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub fn checkpoint_job(&self) -> CheckpointJob {
        self.checkpoint_job_with(self.kms.clone(), self.bucket.clone())
    }

    pub fn checkpoint_job_with(
        &self,
        signer: Arc<dyn ports::KmsSigner>,
        bucket: Arc<dyn hub_identity::audit::CheckpointBucket>,
    ) -> CheckpointJob {
        CheckpointJob::new(CheckpointDeps {
            pool: self.pool.clone(),
            leases: self.leases.clone(),
            signer,
            key: EcdsaKms::key(),
            bucket,
            clock: self.clock.clone() as Arc<dyn Clock>,
            metrics: self.metrics.clone(),
        })
    }

    /// A verifier that trusts this rig's key under its key version.
    pub fn public_key(&self) -> Arc<EcdsaP256Verifier> {
        Arc::new(
            EcdsaP256Verifier::new()
                .with_spki_der(EcdsaKms::key().as_str(), &self.kms.spki_der())
                .unwrap(),
        )
    }

    pub fn chain_verifier(&self) -> ChainVerifier {
        self.chain_verifier_with(self.bucket.clone())
    }

    pub fn chain_verifier_with(
        &self,
        bucket: Arc<dyn hub_identity::audit::CheckpointBucket>,
    ) -> ChainVerifier {
        ChainVerifier::new(self.pool.clone(), bucket, self.public_key())
    }

    pub fn nightly(&self) -> NightlyVerifyJob {
        NightlyVerifyJob::new(
            self.chain_verifier(),
            self.leases.clone(),
            Arc::new(self.notifier.clone()),
            self.metrics.clone(),
        )
    }

    // -----------------------------------------------------------------------------------------------------------
    // What only a database owner can do

    /// Switch off the append-only triggers (and, with `drop_check`, the table's hash check), as an owner with bad intentions
    /// would, to change history. Superuser connection.
    pub async fn tamper_open(&self, drop_check: bool) {
        let mut admin = self.db.admin().await;
        for stmt in [
            "ALTER TABLE audit_events DISABLE TRIGGER audit_events_no_row_change",
            "ALTER TABLE audit_events DISABLE TRIGGER audit_events_no_truncate",
        ] {
            sqlx::query(stmt).execute(&mut admin).await.unwrap();
        }
        if drop_check {
            sqlx::query("ALTER TABLE audit_events DROP CONSTRAINT audit_events_hash_chain")
                .execute(&mut admin)
                .await
                .unwrap();
        }
    }

    /// Run `sql` as the superuser with foreign keys not enforced (the replication role skips their triggers).
    pub async fn admin_sql(&self, sql: &str) {
        let mut admin = self.db.admin().await;
        sqlx::query("SET session_replication_role = replica")
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query(sql).execute(&mut admin).await.unwrap();
    }
}
