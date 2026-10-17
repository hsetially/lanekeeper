//! `SentinelIngest`: the [`SentinelSink`] over Postgres (T7, D72, D74, S5, S25, A16, A17).
//!
//! # `hello`
//!
//! Accepted only for a sentinel an admin registered (`sentinels`, by the VM's name), with the root the admin registered
//! (`export_root`): the name comes from the verified certificate and must equal the payload's, an unknown VM or another root is an
//! `IdentityMismatch` (S5), because the paths of the sentinel's records are mapped by that root (A17). It stores the version and says the
//! sentinel has been seen, and answers with the batch size and the flush interval the sentinel is to use. A sentinel that has said
//! `hello` on this replica is remembered by name, in a table bounded by `sentinel_max_streams`; `records` before it is `HelloRequired`.
//!
//! # `records`
//!
//! One transaction for a batch, in this order, and the acknowledgement is the transaction's commit:
//!
//! 1. the batch is checked against its bounds (records, bytes, a sequence number that is a number the sentinel can have sent) before the
//!    database is touched;
//! 2. the sentinel's row is locked (`UPDATE ... RETURNING`, which also stamps `last_seen_at`), so that two streams of one sentinel (a
//!    reconnect while the old one is still in flight) are applied one at a time;
//! 3. a `seq` **at or below `last_acked_seq`** was applied already: nothing is stored, and the answer is the highest `seq` so far (D74);
//! 4. otherwise the records are mapped to the swimlane's paths ([`super::map`]) and stored (`ON CONFLICT DO NOTHING` on the primary key,
//!    so a duplicate is refused even if the counter were ever reset), the records stored are correlated with the observations they
//!    explain, and `last_acked_seq` moves to `seq`.
//!
//! **What is dropped, and counted, never stored:** a chmod or chown (`Attribute`, A16: it changes no content); every record of a sentinel
//! that serves no swimlane or whose roots cannot be mapped; a record outside the swimlane's prefix (it is another swimlane's, or
//! nobody's); a record whose time is more than `max_clock_skew` ahead of the hub's clock (it would sit in the default partition until
//! the end of time, and no change can be explained by a time that has not come). Old records are stored: a spool replayed after an
//! outage is old, and the default partition's pruning takes what is past its retention.
//!
//! **A gap** (a `seq` more than one above the last) is applied and counted: the sentinel's own spool dropped batches (its cap is 50 MiB),
//! and nothing the hub was given is lost. **A `seq` of 0** is `Invalid`: the schema's 0 means "none acknowledged", so the first batch is 1
//! (a batch numbered 0 would be taken for a resend and lost without a word).
//!
//! # What this never does
//!
//! It never reads or stores file content (a sentinel sends none), puts no path, user name, login or executable in a log line, an error or
//! a metric label (S10), and attributes nothing outside the swimlane the sentinel serves: a compromised sentinel can name any user for a
//! change in its own swimlane within a minute of a record it invents, and nothing else (the threat model states it).

use std::sync::Arc;

use async_trait::async_trait;
use domain::{AuditOperation, AuditRecordBatch, SentinelConfig, SentinelHello, ShortText, SwimlaneId};
use ports::{AckSeq, AuditLog, EventBus, SentinelIdentity, SentinelSink, SinkError};
use sqlx::PgPool;

use super::Inner;
use super::correlate::duration_ms;
use super::grace::AttributionGrace;
use super::map::Mapper;
use super::partitions::SentinelPartitions;
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::error::RegistryError;
use crate::metrics::{Outcome, RegistryMetrics, SentinelKind, SentinelRecord};

/// A sentinel says it is here (`$1` the VM's name, `$2` its version, `$3` the hub's time in ms, `$4` the root it says it watches): only
/// the row an admin registered with that name **and** that root is updated. Served by the unique key on `vm`.
pub const HELLO_SQL: &str = "UPDATE sentinels SET version = $2, \
       last_seen_at = GREATEST(last_seen_at, (timestamptz 'epoch' + ($3::bigint::text || ' milliseconds')::interval)) \
     WHERE vm = $1 AND export_root = $4 RETURNING id";

/// A batch arrives (`$1` the VM's name, `$2` the hub's time in ms): the sentinel's row is locked and stamped, and read with the export of
/// the swimlane it serves. The columns: the sentinel, its swimlane, its root, the last batch acknowledged, the swimlane's export.
pub const SENTINEL_SQL: &str = "WITH s AS (UPDATE sentinels SET \
         last_seen_at = GREATEST(last_seen_at, (timestamptz 'epoch' + ($2::bigint::text || ' milliseconds')::interval)) \
       WHERE vm = $1 RETURNING id, swimlane_id, export_root, last_acked_seq) \
     SELECT s.id, s.swimlane_id, s.export_root, s.last_acked_seq, w.nfs_export \
     FROM s LEFT JOIN swimlanes w ON w.id = s.swimlane_id LIMIT 1";

/// The records of a batch (`$1` the sentinel, `$2` the batch's `seq`, `$3` the hub's time in ms, then one array each: the record's index in
/// the batch, its time in ms, its path under the swimlane, the operation, whether it succeeded, the login user, the effective user, the
/// executable and the command). A record that is stored already (the key is the sentinel, the batch, the index and the time) is not
/// stored again.
pub const INSERT_RECORDS_SQL: &str = "INSERT INTO sentinel_records \
       (sentinel_id, batch_seq, record_index, observed_at, path, operation, success, login_user, effective_user, exe, comm, received_at) \
     SELECT $1, $2, x.i, (timestamptz 'epoch' + (x.t::bigint::text || ' milliseconds')::interval), x.path, x.op, x.ok, \
            x.login, x.eff, x.exe, x.comm, (timestamptz 'epoch' + ($3::bigint::text || ' milliseconds')::interval) \
     FROM unnest($4::int[], $5::bigint[], $6::text[], $7::text[], $8::bool[], $9::text[], $10::text[], $11::text[], $12::text[]) \
          AS x(i, t, path, op, ok, login, eff, exe, comm) \
     ON CONFLICT DO NOTHING";

/// The batch is applied: `last_acked_seq` moves up to it, and never down (`$1` the sentinel, `$2` the `seq`).
pub const ACK_SQL: &str = "UPDATE sentinels SET last_acked_seq = $2 WHERE id = $1 AND last_acked_seq < $2";

/// What the sink needs from the rest of the hub.
pub struct AttributionDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for AttributionDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttributionDeps").finish_non_exhaustive()
    }
}

/// The registry's side of the sentinel protocol. Cloning shares everything.
#[derive(Clone)]
pub struct SentinelIngest {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SentinelIngest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentinelIngest").finish_non_exhaustive()
    }
}

impl SentinelIngest {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: AttributionDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        Ok(Self {
            inner: Arc::new(Inner::new(
                Db::new(deps.pool, deps.config.query_timeout),
                deps.bus,
                deps.audit,
                deps.clock,
                deps.config,
                deps.metrics,
            )),
        })
    }

    /// The leased job that attributes the observations the hub applied after their record, and the ones nothing explains (A19).
    pub fn grace(&self) -> AttributionGrace {
        AttributionGrace::new(Arc::clone(&self.inner))
    }

    /// The leased job that keeps the partitions of `sentinel_records`.
    pub fn partitions(&self) -> SentinelPartitions {
        SentinelPartitions::new(Arc::clone(&self.inner))
    }

    pub fn metrics(&self) -> &RegistryMetrics {
        &self.inner.metrics
    }

    fn done<T>(&self, kind: SentinelKind, result: Result<T, RegistryError>) -> Result<T, SinkError> {
        match result {
            Ok(v) => {
                self.inner.metrics.sentinel_message(kind, Outcome::Ok);
                Ok(v)
            }
            Err(e) => {
                let e = SinkError::from(e);
                self.inner.metrics.sentinel_message(kind, outcome_of(e));
                Err(e)
            }
        }
    }
}

fn outcome_of(e: SinkError) -> Outcome {
    match e {
        SinkError::HelloRequired => Outcome::HelloRequired,
        SinkError::IdentityMismatch => Outcome::IdentityMismatch,
        SinkError::Invalid => Outcome::Invalid,
        SinkError::TooLarge => Outcome::TooLarge,
        SinkError::Unavailable => Outcome::Unavailable,
    }
}

#[async_trait]
impl SentinelSink for SentinelIngest {
    async fn hello(&self, id: &SentinelIdentity, h: SentinelHello) -> Result<SentinelConfig, SinkError> {
        let result = self.inner.hello(id, h).await;
        self.done(SentinelKind::Hello, result)
    }

    async fn records(&self, id: &SentinelIdentity, batch: AuditRecordBatch) -> Result<AckSeq, SinkError> {
        let result = self.inner.records(id, batch).await;
        self.done(SentinelKind::Records, result)
    }
}

/// A record that is to be stored: its index in the batch, its time and its path under the swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub index: i32,
    pub at_ms: i64,
    pub path: String,
    pub operation: &'static str,
    pub success: bool,
    pub login: Option<String>,
    pub effective: Option<String>,
    pub exe: Option<String>,
    pub comm: Option<String>,
}

/// What a batch comes to: the records to store, and the count of each kind that is not stored.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Planned {
    pub stored: Vec<Stored>,
    pub attribute: u64,
    pub unserved: u64,
    pub future: u64,
    pub outside: u64,
}

impl Planned {
    /// Whether anything stored can explain a change (a record that succeeded): otherwise there is nothing to correlate.
    pub fn correlates(&self) -> bool {
        self.stored.iter().any(|s| s.success)
    }
}

fn non_empty(text: &ShortText) -> Option<String> {
    (!text.as_str().is_empty()).then(|| text.as_str().to_owned())
}

/// What becomes of each record of `batch`, decided without the database. `mapper` is `None` when the sentinel serves no swimlane or its
/// roots cannot be mapped (A17). The order of the reasons is fixed: a chmod or chown first (it is never stored), then a sentinel that
/// serves nothing, then a time in the future, then a path outside the swimlane.
pub(crate) fn plan(batch: &AuditRecordBatch, mapper: Option<&Mapper>, now_ms: i64, skew_ms: i64) -> Planned {
    let mut out = Planned::default();
    for (i, r) in batch.records.iter().enumerate() {
        let operation = match r.operation {
            AuditOperation::Create => "create",
            AuditOperation::Write => "write",
            AuditOperation::Delete => "delete",
            AuditOperation::Rename => "rename",
            AuditOperation::Attribute => {
                out.attribute += 1;
                continue;
            }
        };
        let Some(mapper) = mapper else {
            out.unserved += 1;
            continue;
        };
        let at_ms = r.time.unix_millis();
        if at_ms > now_ms.saturating_add(skew_ms) {
            out.future += 1;
            continue;
        }
        let Some(path) = mapper.map(&r.path) else {
            out.outside += 1;
            continue;
        };
        let Ok(index) = i32::try_from(i) else {
            // Cannot happen under the batch bound (at most 10,000 records); counted with what is dropped for the same kind of reason.
            out.outside += 1;
            continue;
        };
        out.stored.push(Stored {
            index,
            at_ms,
            path: path.as_str().to_owned(),
            operation,
            success: r.success,
            login: non_empty(&r.login_user),
            effective: non_empty(&r.effective_user),
            exe: non_empty(&r.exe),
            comm: non_empty(&r.comm),
        });
    }
    out
}

/// The bytes of text in a batch, with a fixed cost for each record: what `sentinel_max_batch_bytes` bounds.
pub(crate) fn text_bytes(batch: &AuditRecordBatch) -> usize {
    batch.records.iter().fold(0_usize, |sum, r| {
        sum.saturating_add(
            64 + r.path.as_str().len()
                + r.login_user.as_str().len()
                + r.effective_user.as_str().len()
                + r.exe.as_str().len()
                + r.comm.as_str().len(),
        )
    })
}

/// One row of [`SENTINEL_SQL`]: the sentinel, its swimlane, its root, the last batch acknowledged, the swimlane's export.
type SentinelRow = (i64, Option<String>, String, i64, Option<String>);

impl Inner {
    pub(crate) async fn hello(
        &self,
        id: &SentinelIdentity,
        h: SentinelHello,
    ) -> Result<SentinelConfig, RegistryError> {
        // S5: a sentinel can only act for itself, whatever the payload says.
        if &h.vm != id.name() {
            return Err(RegistryError::IdentityMismatch);
        }
        // Room first, so that a refused hello writes nothing, and the place goes back if the hello then fails (a name nobody registered must
        // not use one up).
        let new_place = self.admit(id.name())?;
        let result = self.join(id.name(), &h).await;
        if result.is_err() && new_place {
            self.forget(id.name());
        }
        result
    }

    async fn join(&self, name: &ShortText, h: &SentinelHello) -> Result<SentinelConfig, RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let known = self
            .db
            .run(
                "sentinel_hello",
                sqlx::query(HELLO_SQL)
                    .bind(name.as_str())
                    .bind(h.version.as_str())
                    .bind(now_ms)
                    .bind(h.export_root.as_str())
                    .execute(self.db.pool()),
            )
            .await?;
        if known.rows_affected() == 0 {
            // Not registered by an admin, or watching another root than the admin registered.
            return Err(RegistryError::IdentityMismatch);
        }
        Ok(SentinelConfig {
            max_batch_records: self.config.sentinel_max_batch_records,
            flush_interval_secs: self.config.sentinel_flush_interval_secs,
        })
    }

    pub(crate) async fn records(
        &self,
        id: &SentinelIdentity,
        batch: AuditRecordBatch,
    ) -> Result<AckSeq, RegistryError> {
        let name = id.name();
        if !self.said_hello(name) {
            return Err(RegistryError::HelloRequired);
        }
        // The bounds first (rule 5): a batch over them is refused before anything is read, locked or stored.
        let records = u64::try_from(batch.records.len()).unwrap_or(u64::MAX);
        if batch.records.len() > usize::try_from(self.config.sentinel_max_batch_records).unwrap_or(usize::MAX)
            || text_bytes(&batch) > self.config.sentinel_max_batch_bytes
        {
            return Err(RegistryError::TooLarge);
        }
        // The first batch is 1: the schema's 0 means that none was acknowledged, so a batch numbered 0 would be taken for a resend.
        let seq = i64::try_from(batch.seq)
            .ok()
            .filter(|s| *s >= 1)
            .ok_or(RegistryError::Invalid)?;

        let now_ms = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;
        let row: Option<SentinelRow> = self
            .db
            .run(
                "sentinel_batch",
                sqlx::query_as(SENTINEL_SQL)
                    .bind(name.as_str())
                    .bind(now_ms)
                    .fetch_optional(&mut *tx),
            )
            .await?;
        let Some((sentinel, lane, export_root, acked, export)) = row else {
            // The sentinel was removed after its hello.
            return Err(RegistryError::IdentityMismatch);
        };

        if seq <= acked {
            // Applied already (at-least-once delivery, D74): acknowledged with the highest `seq` so far and stored again by no one. The
            // transaction commits for the stamp that says the sentinel is here.
            self.db.commit(tx).await?;
            self.metrics.sentinel_record(SentinelRecord::Resent, records);
            return Ok(AckSeq::new(u64::try_from(acked).unwrap_or(0)));
        }
        let gap = seq > acked.saturating_add(1);

        let lane = lane.and_then(|l| SwimlaneId::parse(&l).ok());
        let mapper = lane
            .as_ref()
            .and_then(|_| Mapper::new(&export_root, export.as_deref()));
        let planned = plan(
            &batch,
            mapper.as_ref(),
            now_ms,
            duration_ms(self.config.max_clock_skew),
        );
        if !planned.stored.is_empty() {
            self.store(&mut tx, sentinel, seq, now_ms, &planned).await?;
        }
        let upgraded = match &lane {
            Some(lane) if planned.correlates() => {
                self.attribute_batch(&mut tx, sentinel, name, lane, seq).await?
            }
            _ => 0,
        };
        self.db
            .run(
                "sentinel_ack",
                sqlx::query(ACK_SQL).bind(sentinel).bind(seq).execute(&mut *tx),
            )
            .await?;
        self.db.commit(tx).await?;

        // The counts, once the batch is durable. Nothing in them names a path, a user or an executable.
        if gap {
            self.metrics.sentinel_gap();
        }
        let stored = u64::try_from(planned.stored.len()).unwrap_or(u64::MAX);
        self.metrics.sentinel_record(SentinelRecord::Stored, stored);
        self.metrics
            .sentinel_record(SentinelRecord::Attribute, planned.attribute);
        self.metrics
            .sentinel_record(SentinelRecord::Unserved, planned.unserved);
        self.metrics
            .sentinel_record(SentinelRecord::Future, planned.future);
        self.metrics
            .sentinel_record(SentinelRecord::Outside, planned.outside);
        tracing::debug!(
            stored,
            attribute = planned.attribute,
            unserved = planned.unserved,
            future = planned.future,
            outside = planned.outside,
            upgraded,
            gap,
            "sentinel batch applied"
        );
        Ok(AckSeq::new(u64::try_from(seq).unwrap_or(0)))
    }

    /// Store the records of a batch that are the swimlane's.
    async fn store(
        &self,
        tx: &mut crate::db::Txn,
        sentinel: i64,
        seq: i64,
        now_ms: i64,
        planned: &Planned,
    ) -> Result<(), RegistryError> {
        let s = &planned.stored;
        let index: Vec<i32> = s.iter().map(|r| r.index).collect();
        let at: Vec<i64> = s.iter().map(|r| r.at_ms).collect();
        let path: Vec<&str> = s.iter().map(|r| r.path.as_str()).collect();
        let op: Vec<&str> = s.iter().map(|r| r.operation).collect();
        let ok: Vec<bool> = s.iter().map(|r| r.success).collect();
        let login: Vec<Option<&str>> = s.iter().map(|r| r.login.as_deref()).collect();
        let eff: Vec<Option<&str>> = s.iter().map(|r| r.effective.as_deref()).collect();
        let exe: Vec<Option<&str>> = s.iter().map(|r| r.exe.as_deref()).collect();
        let comm: Vec<Option<&str>> = s.iter().map(|r| r.comm.as_deref()).collect();
        self.db
            .run(
                "sentinel_records",
                sqlx::query(INSERT_RECORDS_SQL)
                    .bind(sentinel)
                    .bind(seq)
                    .bind(now_ms)
                    .bind(&index)
                    .bind(&at)
                    .bind(&path)
                    .bind(&op)
                    .bind(&ok)
                    .bind(&login)
                    .bind(&eff)
                    .bind(&exe)
                    .bind(&comm)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use domain::{AuditRecord, NfsPath, Timestamp};

    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const SKEW: i64 = 300_000;

    fn text(s: &str) -> ShortText {
        ShortText::parse(s).unwrap()
    }

    fn record(path: &str, op: AuditOperation, at_ms: i64) -> AuditRecord {
        AuditRecord {
            time: Timestamp::from_unix_millis(at_ms),
            path: NfsPath::parse(path).unwrap(),
            operation: op,
            success: true,
            login_user: text("ada_example_com"),
            effective_user: text("root"),
            exe: text("/usr/bin/vim"),
            comm: text("vim"),
        }
    }

    fn batch(records: Vec<AuditRecord>) -> AuditRecordBatch {
        AuditRecordBatch { seq: 1, records }
    }

    fn mapper() -> Mapper {
        Mapper::new("/export", Some("/export/sit1")).unwrap()
    }

    #[test]
    fn a_record_of_the_swimlane_is_stored_under_its_path_and_keeps_its_index() {
        let b = batch(vec![
            record("sit2/a.yml", AuditOperation::Write, NOW),
            record("sit1/svc/a.yml", AuditOperation::Write, NOW),
            record("sit1/svc/b.yml", AuditOperation::Rename, NOW - 5),
        ]);
        let p = plan(&b, Some(&mapper()), NOW, SKEW);
        assert_eq!(p.outside, 1);
        assert_eq!(
            p.stored
                .iter()
                .map(|s| (s.index, s.path.as_str(), s.operation))
                .collect::<Vec<_>>(),
            vec![(1, "svc/a.yml", "write"), (2, "svc/b.yml", "rename")],
            "the index is the record's place in the batch, so a replay of the batch keeps every key"
        );
        assert!(p.correlates());
    }

    #[test]
    fn a_chmod_or_chown_is_counted_and_never_stored_whatever_its_path() {
        let b = batch(vec![
            record("sit1/a.yml", AuditOperation::Attribute, NOW),
            record("sit2/a.yml", AuditOperation::Attribute, NOW),
        ]);
        let p = plan(&b, Some(&mapper()), NOW, SKEW);
        assert_eq!((p.attribute, p.outside, p.stored.len()), (2, 0, 0));
        assert!(!p.correlates());
        let p = plan(&b, None, NOW, SKEW);
        assert_eq!((p.attribute, p.unserved), (2, 0), "a chmod is a chmod first");
    }

    #[test]
    fn a_sentinel_that_serves_nothing_stores_nothing() {
        let b = batch(vec![
            record("sit1/a.yml", AuditOperation::Write, NOW),
            record("sit1/b.yml", AuditOperation::Delete, NOW),
        ]);
        let p = plan(&b, None, NOW, SKEW);
        assert_eq!((p.unserved, p.stored.len()), (2, 0));
    }

    #[test]
    fn a_time_in_the_future_is_dropped_by_more_than_the_skew_and_kept_at_it() {
        let b = batch(vec![
            record("sit1/a.yml", AuditOperation::Write, NOW + SKEW),
            record("sit1/b.yml", AuditOperation::Write, NOW + SKEW + 1),
            record("sit1/c.yml", AuditOperation::Write, i64::MAX),
        ]);
        let p = plan(&b, Some(&mapper()), NOW, SKEW);
        assert_eq!((p.stored.len(), p.future), (1, 2));
        assert_eq!(p.stored[0].path, "a.yml");
    }

    #[test]
    fn an_old_record_is_stored_because_a_replayed_spool_is_old() {
        let b = batch(vec![record("sit1/a.yml", AuditOperation::Write, 10)]);
        assert_eq!(plan(&b, Some(&mapper()), NOW, SKEW).stored.len(), 1);
    }

    #[test]
    fn a_record_that_failed_is_stored_and_explains_nothing() {
        let mut r = record("sit1/a.yml", AuditOperation::Write, NOW);
        r.success = false;
        let p = plan(&batch(vec![r]), Some(&mapper()), NOW, SKEW);
        assert_eq!(p.stored.len(), 1);
        assert!(!p.stored[0].success);
        assert!(!p.correlates(), "nothing that succeeded, nothing to correlate");
    }

    #[test]
    fn empty_names_are_stored_as_nothing() {
        let mut r = record("sit1/a.yml", AuditOperation::Write, NOW);
        r.login_user = text("");
        r.exe = text("");
        let p = plan(&batch(vec![r]), Some(&mapper()), NOW, SKEW);
        assert_eq!(p.stored[0].login, None);
        assert_eq!(p.stored[0].exe, None);
        assert_eq!(p.stored[0].effective.as_deref(), Some("root"));
    }

    #[test]
    fn the_bytes_of_a_batch_count_every_text_and_a_cost_for_each_record() {
        assert_eq!(text_bytes(&batch(Vec::new())), 0);
        let one = batch(vec![record("a.yml", AuditOperation::Write, NOW)]);
        assert_eq!(
            text_bytes(&one),
            64 + "a.yml".len() + "ada_example_com".len() + "root".len() + "/usr/bin/vim".len() + "vim".len()
        );
        let many = batch(
            (0..10)
                .map(|_| record("a.yml", AuditOperation::Write, NOW))
                .collect(),
        );
        assert_eq!(text_bytes(&many), 10 * text_bytes(&one));
    }

    #[test]
    fn the_statements_use_the_exact_time_expressions_and_the_sink_never_moves_the_counter_down() {
        use crate::db::ts;
        assert!(HELLO_SQL.contains(&ts("$3")));
        assert!(SENTINEL_SQL.contains(&ts("$2")));
        assert!(INSERT_RECORDS_SQL.contains(&ts("$3")));
        assert!(INSERT_RECORDS_SQL.contains(&ts("x.t")));
        assert!(INSERT_RECORDS_SQL.contains("ON CONFLICT DO NOTHING"));
        assert!(ACK_SQL.contains("last_acked_seq < $2"));
        assert!(
            HELLO_SQL.contains("export_root = $4"),
            "the root the admin registered"
        );
    }
}
