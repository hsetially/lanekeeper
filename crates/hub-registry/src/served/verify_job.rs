//! The leased job `registry-served-verify` (T10, D86, A36): the nightly check of the engine against the config-server. What it compares, how
//! the sample is chosen and what a finding says are in [`super`]; this file is how it runs.
//!
//! - **One job, one lease.** [`ServedVerifier::spec`] schedules it every `served_verify_interval` (a day) under a 90 second lease that the
//!   runner renews; `run_once` is the same run for a test. Whoever holds the lease runs it, and the job asks the lease before every request
//!   and stops when it is lost.
//! - **Only swimlanes whose agent is connected.** The others are skipped and counted (A36): the check asks the agent, and an agent that is
//!   not there cannot answer. A swimlane with an open C11 finding is skipped and counted too ([`super`]).
//! - **One request at a time per swimlane**, `served_verify_pause` apart (250 ms by default: under the agent's own limit of five a
//!   second), each with `served_timeout` (10 s). At most `served_verify_lanes` swimlanes are asked at once. A request takes a slot of the
//!   same `served_slots` the REST route and the MCP tool use (16 a replica), waiting for one up to `served_verify_slot_wait` and no longer,
//!   so a night of checks neither starves a person who asked for a served file nor is refused by one.
//! - **The same round trip as the route**: [`Inner::round_trip`](crate::read::service::Inner), the function `getServedFile` is made of; the
//!   timeout, the command and the limits are not repeated here. Only a `FetchServed` is ever sent (a test reads the commands the agent got).
//! - **Findings** go through [`findings_store`](crate::pipeline::findings_store), the writer C9 to C11 use: one transaction under the
//!   swimlane's findings lock, `FindingsChanged` through the outbox, nothing written when nothing differs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    AgentStatus, AppName, ChannelName, ContentHash, FileClass, FileRole, FindingKind, LogicalFile, NfsPath,
    ServeRequest, SwimlaneId, TenantId,
};
use engine::{ChangeContext, normalised_hash, severity};
use futures::stream::{self, StreamExt};
use hub_platform::jobs::{Job, JobContext, JobError, JobSpec, SpecError};
use ports::EventBus;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use super::{Combination, Population, sample};
use crate::config::RegistryConfig;
use crate::metrics::{RegistryMetrics, ServedResult, ServedSwimlane};
use crate::pipeline::c9_c11::inputs::{self, Gate};
use crate::pipeline::findings_store::{Rows, Store, reconcile, stored};
use crate::pipeline::findings_sync::{FindingRow, fingerprint_keyed};
use crate::read::served::{Round, Unreached};
use crate::read::service::Inner;
use crate::read::{RegistryService, Tagged};
use crate::severity::SeverityRuleSource;

/// The job's name, which is also its lease's name.
pub const JOB_NAME: &str = "registry-served-verify";

/// The kinds of finding this job owns.
const KINDS: [&str; 1] = ["served_mismatch"];

/// The most swimlanes one run looks at (the registry holds tens; the bound is a guard, rule 5).
const MAX_LANES: usize = 10_000;

/// What the job needs from the rest of the hub.
pub struct ServedDeps {
    /// The read service: its database, gateway, blob store, limits, deny list and slots are the ones the routes use.
    pub service: RegistryService,
    pub bus: Arc<dyn EventBus>,
    /// The rules findings are rated with (the same source the pipeline uses).
    pub severity: SeverityRuleSource,
}

impl std::fmt::Debug for ServedDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServedDeps").finish_non_exhaustive()
    }
}

/// What one run did, in counts (S10: nothing of a file).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServedReport {
    /// Swimlanes asked.
    pub swimlanes: usize,
    /// Swimlanes skipped because their agent is not connected.
    pub offline: usize,
    /// Swimlanes skipped because the config-server has to restart before it serves what is on NFS (an open C11).
    pub restart_pending: usize,
    /// Swimlanes that could not be set up (a database or blob store that did not answer): the next run asks again.
    pub failed: usize,
    /// `FetchServed` requests sent.
    pub requests: usize,
    pub matched: usize,
    pub mismatched: usize,
    /// The config-server answered "not found" for a file the engine has.
    pub not_served: usize,
    /// The agent did not answer within the timeout.
    pub timeouts: usize,
    /// The agent refused the path (its deny list): nothing was asked of the config-server.
    pub denied: usize,
    /// Combinations that could not be compared: a status other than 200 or 404, a reply that was not a served file, a slot that did not
    /// come free, an agent that went away, or an engine that had no rendering to hold the answer against.
    pub unverified: usize,
    pub inserted: usize,
    pub updated: usize,
    pub resolved: usize,
}

impl ServedReport {
    fn add(&mut self, o: &Self) {
        self.swimlanes += o.swimlanes;
        self.offline += o.offline;
        self.restart_pending += o.restart_pending;
        self.failed += o.failed;
        self.requests += o.requests;
        self.matched += o.matched;
        self.mismatched += o.mismatched;
        self.not_served += o.not_served;
        self.timeouts += o.timeouts;
        self.denied += o.denied;
        self.unverified += o.unverified;
        self.inserted += o.inserted;
        self.updated += o.updated;
        self.resolved += o.resolved;
    }

    /// Findings written (new, changed or resolved).
    pub fn written(&self) -> usize {
        self.inserted + self.updated + self.resolved
    }
}

/// What the check came to for one combination.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Match,
    /// The config-server serves something else (`served`), or nothing (`None`).
    Mismatch {
        engine: ContentHash,
        served: Option<ContentHash>,
    },
    /// Nothing can be said: whatever stood before stays.
    Unverified,
}

/// The nightly check of the engine against the config-server. Cloning shares it.
#[derive(Clone)]
pub struct ServedVerifier {
    read: Arc<Inner>,
    bus: Arc<dyn EventBus>,
    severity: SeverityRuleSource,
}

impl std::fmt::Debug for ServedVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServedVerifier").finish_non_exhaustive()
    }
}

impl ServedVerifier {
    pub fn new(deps: ServedDeps) -> Self {
        Self {
            read: Arc::clone(&deps.service.inner),
            bus: deps.bus,
            severity: deps.severity,
        }
    }

    /// How the job is scheduled: every `served_verify_interval`, under a 90 second lease that the runner renews, for at most an hour.
    ///
    /// # Errors
    ///
    /// The interval or the name is not one a [`JobSpec`] takes (the interval is range-checked by `RegistryConfig::validate`).
    pub fn spec(config: &RegistryConfig) -> Result<JobSpec, SpecError> {
        JobSpec::new(JOB_NAME, config.served_verify_interval, Duration::from_secs(90))?
            .with_max_run(Duration::from_secs(3600))
    }

    /// The job (`registry-served-verify`), for the hub's job runner.
    pub fn job(&self) -> Arc<dyn Job> {
        Arc::new(VerifyJob {
            verifier: self.clone(),
        })
    }

    /// One run, as a test calls it: the lease is taken to be held.
    ///
    /// # Errors
    ///
    /// `Failed` when the swimlanes cannot be listed.
    pub async fn run_once(&self) -> Result<ServedReport, JobError> {
        self.verify(|| Ok(())).await
    }

    /// One run. `held` is asked before every request and ends the run with its error.
    ///
    /// # Errors
    ///
    /// `LeaseLost` from `held`; `Failed` when the swimlanes cannot be listed.
    pub async fn verify(
        &self,
        held: impl Fn() -> Result<(), JobError> + Send + Sync,
    ) -> Result<ServedReport, JobError> {
        held()?;
        let gates = inputs::gates(&self.read.db, None, MAX_LANES)
            .await
            .map_err(|_| JobError::Failed("registry_served_verify"))?;
        let lanes: Vec<Gate> = gates.into_iter().filter(Gate::ready).take(MAX_LANES).collect();
        let parallel = self.read.config.served_verify_lanes.max(1);
        let outcomes: Vec<Result<ServedReport, JobError>> = stream::iter(lanes)
            .map(|gate| self.verify_lane(gate, &held))
            .buffer_unordered(parallel)
            .collect()
            .await;
        let mut report = ServedReport::default();
        let mut lost = None;
        for outcome in outcomes {
            match outcome {
                Ok(lane) => report.add(&lane),
                Err(e) => lost = Some(e),
            }
        }
        if let Some(e) = lost {
            return Err(e);
        }
        info!(
            swimlanes = report.swimlanes,
            offline = report.offline,
            restart_pending = report.restart_pending,
            failed = report.failed,
            requests = report.requests,
            mismatched = report.mismatched,
            timeouts = report.timeouts,
            written = report.written(),
            "served verification"
        );
        Ok(report)
    }

    /// One swimlane. A lost lease is the only error; anything else that goes wrong is counted as `failed` and asked again next run.
    async fn verify_lane(
        &self,
        gate: Gate,
        held: &(impl Fn() -> Result<(), JobError> + Sync),
    ) -> Result<ServedReport, JobError> {
        let swimlane = gate.swimlane;
        let mut report = ServedReport::default();
        if self.read.gateway.status(&swimlane).await != AgentStatus::Connected {
            report.offline = 1;
            self.read.metrics.served_verify_swimlane(ServedSwimlane::Offline);
            return Ok(report);
        }
        let prepared = match self.prepare(&swimlane).await {
            Ok(Prepared::Ready(p)) => *p,
            Ok(Prepared::RestartPending) => {
                report.restart_pending = 1;
                self.read
                    .metrics
                    .served_verify_swimlane(ServedSwimlane::RestartPending);
                return Ok(report);
            }
            Err(_) => {
                warn!(swimlane = %swimlane, "the served verification could not be set up for a swimlane");
                report.failed = 1;
                self.read.metrics.served_verify_swimlane(ServedSwimlane::Failed);
                return Ok(report);
            }
        };
        report.swimlanes = 1;
        self.read.metrics.served_verify_swimlane(ServedSwimlane::Verified);
        let pause = self.read.config.served_verify_pause;
        let mut verdicts: BTreeMap<String, (Combination, Verdict)> = BTreeMap::new();
        for (i, combination) in prepared.sample.iter().enumerate() {
            held()?;
            if i > 0 && !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
            let verdict = self
                .check(&swimlane, combination, &prepared.classes, &mut report)
                .await;
            verdicts.insert(combination.key(), (combination.clone(), verdict));
        }
        held()?;
        if let Ok((inserted, updated, resolved)) = self.write(&swimlane, &prepared, &verdicts).await {
            report.inserted = inserted;
            report.updated = updated;
            report.resolved = resolved;
        } else {
            warn!(swimlane = %swimlane, "the findings of the served verification could not be written");
            report.failed = 1;
            self.read.metrics.served_verify_swimlane(ServedSwimlane::Failed);
        }
        Ok(report)
    }

    /// What a swimlane's run is made of: its sample and the classes of its files. `RestartPending` when an open C11 says that the
    /// config-server is not serving what is on NFS.
    async fn prepare(&self, swimlane: &SwimlaneId) -> Result<Prepared, SetupFailed> {
        let read = &self.read;
        if !stored(&read.db, swimlane, &["config_server_restart_required"])
            .await?
            .is_empty()
        {
            return Ok(Prepared::RestartPending);
        }
        let files = inputs::files(&read.db, swimlane).await?;
        let services = inputs::services(&read.db, swimlane).await?;
        let apps: Vec<AppName> = services.into_iter().filter_map(|s| s.app).collect();
        let tenants = read.tenants_of(swimlane).await?;
        let channels = read.listed_channels(swimlane).await?;
        let deny = |f: &inputs::FileRow| crate::denied::is_denied(&read.deny, &f.path, f.denied, f.class);
        let max_bytes = read.config.max_file_bytes;
        let population = Population::new(&files, &deny, max_bytes, tenants, apps, &channels);
        let before = stored(&read.db, swimlane, &KINDS).await?;
        let pinned: Vec<Combination> = before
            .values()
            .filter_map(|r| combination_of(&r.details))
            .collect();
        let interval = u64::try_from(read.config.served_verify_interval.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        let round = u64::try_from(read.clock.now().unix_millis()).unwrap_or(0) / interval;
        let chosen = sample(
            &population,
            swimlane,
            round,
            &pinned,
            read.config.served_verify_sample,
        );
        let classes: BTreeMap<NfsPath, FileClass> = files.iter().map(|f| (f.path.clone(), f.class)).collect();
        Ok(Prepared::Ready(Box::new(Run {
            population,
            sample: chosen,
            classes,
        })))
    }

    /// One combination: the engine's rendering, then, if there is one, the config-server's answer for it, held against each other.
    async fn check(
        &self,
        swimlane: &SwimlaneId,
        c: &Combination,
        classes: &BTreeMap<NfsPath, FileClass>,
        report: &mut ServedReport,
    ) -> Verdict {
        let read = &self.read;
        let metrics: &RegistryMetrics = &read.metrics;
        let count = |result: ServedResult| metrics.served_verify_combination(result);
        // The engine first: a file it has no rendering for (denied, gone, not a resource) is never asked of the agent.
        let engine = match read
            .effective_config(swimlane, &c.file, Some(&c.tenant), c.channel.as_ref(), None)
            .await
        {
            Ok(Tagged::Fresh { value, .. }) if value.role == FileRole::Resource => value,
            _ => {
                report.unverified += 1;
                count(ServedResult::Engine);
                return Verdict::Unverified;
            }
        };
        let Some(class) = engine.sources.first().and_then(|s| classes.get(&s.path)).copied() else {
            report.unverified += 1;
            count(ServedResult::Engine);
            return Verdict::Unverified;
        };
        if class == FileClass::Denied {
            report.unverified += 1;
            count(ServedResult::Engine);
            return Verdict::Unverified;
        }
        // A slot of the replica's served requests, waited for up to the timeout.
        let slot =
            tokio::time::timeout(read.config.served_verify_slot_wait, read.served_slots.acquire()).await;
        let Ok(Ok(_slot)) = slot else {
            report.unverified += 1;
            count(ServedResult::Busy);
            return Verdict::Unverified;
        };
        let request = ServeRequest {
            application: c.application.clone(),
            tenant: c.tenant.clone(),
            channel: c.channel.clone(),
            file: c.file.as_path().clone(),
        };
        report.requests += 1;
        match read.round_trip(swimlane, &request).await {
            Ok(Round::Served(bytes)) => {
                // Hashed and dropped: the served text goes nowhere (S10).
                let served = normalised_hash(&bytes, class);
                if served == engine.hash {
                    report.matched += 1;
                    count(ServedResult::Match);
                    Verdict::Match
                } else {
                    report.mismatched += 1;
                    count(ServedResult::Mismatch);
                    Verdict::Mismatch {
                        engine: engine.hash,
                        served: Some(served),
                    }
                }
            }
            Ok(Round::Status(404)) => {
                report.not_served += 1;
                count(ServedResult::NotServed);
                Verdict::Mismatch {
                    engine: engine.hash,
                    served: None,
                }
            }
            Ok(Round::Denied) => {
                report.denied += 1;
                count(ServedResult::Denied);
                Verdict::Unverified
            }
            Ok(Round::Status(_) | Round::Other)
            | Err(Unreached::NotConnected | Unreached::Closed | Unreached::Busy | Unreached::Unavailable) => {
                report.unverified += 1;
                count(ServedResult::Agent);
                Verdict::Unverified
            }
            Err(Unreached::Timeout) => {
                report.timeouts += 1;
                count(ServedResult::Timeout);
                Verdict::Unverified
            }
        }
    }

    /// Make the stored C12 findings of the swimlane say what the verdicts say: a mismatch is raised (or updated), a match clears its
    /// finding, and what was not verified stays as it was, unless its combination is gone.
    async fn write(
        &self,
        swimlane: &SwimlaneId,
        run: &Run,
        verdicts: &BTreeMap<String, (Combination, Verdict)>,
    ) -> Result<(usize, usize, usize), crate::error::RegistryError> {
        let rules = self.severity.current().await?;
        let mut raised: Rows = Rows::new();
        for (combination, verdict) in verdicts.values() {
            if let Verdict::Mismatch { engine, served } = verdict {
                let row = served_row(&rules.rules, combination, *engine, *served);
                raised.insert((FindingKind::ServedMismatch, row.fingerprint.clone()), row);
            }
        }
        let store = Store {
            db: &self.read.db,
            bus: &*self.bus,
            clock: &*self.read.clock,
            rows: self.read.config.rows_per_statement,
        };
        let done = reconcile(&store, swimlane, &KINDS, |held| {
            let mut out: Rows = Rows::new();
            for (key, row) in held {
                let Some(c) = combination_of(&row.details) else {
                    // Not a finding this job could have written: it cannot be verified, so it does not stay.
                    continue;
                };
                let keep = match verdicts.get(&c.key()).map(|(_, v)| v) {
                    Some(Verdict::Match | Verdict::Mismatch { .. }) => false,
                    Some(Verdict::Unverified) | None => run.population.contains(&c),
                };
                if keep {
                    out.insert(key.clone(), row.clone());
                }
            }
            out.extend(raised.iter().map(|(k, v)| (k.clone(), v.clone())));
            out
        })
        .await?;
        debug!(
            inserted = done.inserted,
            updated = done.updated,
            resolved = done.resolved,
            "served findings"
        );
        Ok((done.inserted, done.updated, done.resolved))
    }
}

/// A swimlane's run: what it needs to know of the swimlane, read once.
struct Run {
    population: Population,
    sample: Vec<Combination>,
    classes: BTreeMap<NfsPath, FileClass>,
}

enum Prepared {
    Ready(Box<Run>),
    RestartPending,
}

/// A swimlane's run could not be set up: something it reads did not answer. Nothing of why is kept (S21); the next run asks again.
#[derive(Debug)]
struct SetupFailed;

impl From<crate::error::RegistryError> for SetupFailed {
    fn from(_: crate::error::RegistryError) -> Self {
        Self
    }
}

impl From<ports::ReadError> for SetupFailed {
    fn from(_: ports::ReadError) -> Self {
        Self
    }
}

/// The finding of a combination that is served differently from what the engine renders. Names and hashes of whole files, no content and
/// no value (S10): `served_hash` is `null` when the config-server did not serve the file at all.
fn served_row(
    rules: &engine::SeverityRules,
    c: &Combination,
    engine_hash: ContentHash,
    served: Option<ContentHash>,
) -> FindingRow {
    let outcome = severity(
        &ChangeContext {
            finding_kind: Some(FindingKind::ServedMismatch),
            path: Some(c.file.as_path()),
            ..ChangeContext::default()
        },
        rules,
    );
    let channel = c
        .channel
        .as_ref()
        .map_or_else(String::new, |ch| format!(" in channel {ch}"));
    let message = match served {
        Some(_) => format!(
            "The config-server serves {} for tenant {}{channel} differently from the engine's rendering",
            c.file, c.tenant
        ),
        None => format!(
            "The config-server does not serve {} for tenant {}{channel}, which the engine has a rendering for",
            c.file, c.tenant
        ),
    };
    FindingRow {
        kind: FindingKind::ServedMismatch,
        path: Some(c.file.as_str().to_owned()),
        severity: outcome.severity,
        rule_ids: outcome.rule_ids.iter().map(|r| r.as_str().to_owned()).collect(),
        message,
        locations: json!([]),
        details: json!({
            "application": c.application.as_str(),
            "tenant": c.tenant.as_str(),
            "channel": c.channel.as_ref().map(ChannelName::as_str),
            "file": c.file.as_str(),
            "reason": if served.is_some() { "differs" } else { "not_served" },
            "engine_hash": engine_hash.to_string(),
            "served_hash": served.map(|h| h.to_string()),
        }),
        fingerprint: fingerprint_keyed(FindingKind::ServedMismatch, &c.key()),
    }
}

/// The combination a stored finding is about, from its details; `None` for details that are not ours.
fn combination_of(details: &Value) -> Option<Combination> {
    let text = |key: &str| details.get(key)?.as_str();
    Some(Combination {
        application: AppName::parse(text("application")?).ok()?,
        tenant: TenantId::parse(text("tenant")?).ok()?,
        channel: match details.get("channel")? {
            Value::Null => None,
            Value::String(s) => Some(ChannelName::parse(s).ok()?),
            _ => return None,
        },
        file: LogicalFile::parse(text("file")?).ok()?,
    })
}

struct VerifyJob {
    verifier: ServedVerifier,
}

#[async_trait]
impl Job for VerifyJob {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        self.verifier.verify(|| ctx.ensure_held()).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use domain::{AppName, Severity};

    use super::*;

    fn combination(channel: Option<&str>) -> Combination {
        Combination {
            application: AppName::parse("core").unwrap(),
            tenant: TenantId::parse("sit1").unwrap(),
            channel: channel.map(|c| ChannelName::parse(c).unwrap()),
            file: LogicalFile::parse("core/msg.txt").unwrap(),
        }
    }

    fn rules() -> engine::SeverityRules {
        engine::SeverityRules::new(crate::severity::all_defaults()).unwrap()
    }

    #[test]
    fn the_job_is_named_after_its_lease_and_the_default_config_makes_a_valid_spec() {
        let spec = ServedVerifier::spec(&RegistryConfig::default()).unwrap();
        assert_eq!(spec.name(), JOB_NAME);
        assert_eq!(JOB_NAME, "registry-served-verify");
        assert!(ports::is_valid_lease_name(JOB_NAME));
    }

    #[test]
    fn a_mismatch_is_high_names_the_file_and_holds_hashes_and_no_content() {
        let engine = ContentHash::from_bytes([1; 32]);
        let served = ContentHash::from_bytes([2; 32]);
        let row = served_row(&rules(), &combination(Some("web")), engine, Some(served));
        assert_eq!(row.kind, FindingKind::ServedMismatch);
        assert_eq!(row.severity, Severity::High);
        assert_eq!(row.path.as_deref(), Some("core/msg.txt"));
        assert!(
            row.message.contains("core/msg.txt")
                && row.message.contains("sit1")
                && row.message.contains("web")
        );
        assert_eq!(row.details["reason"], "differs");
        assert_eq!(row.details["engine_hash"], engine.to_string());
        assert_eq!(row.details["served_hash"], served.to_string());
        let none = served_row(&rules(), &combination(None), engine, None);
        assert_eq!(none.details["reason"], "not_served");
        assert!(none.details["served_hash"].is_null() && none.details["channel"].is_null());
        assert_ne!(
            row.fingerprint, none.fingerprint,
            "the channel is part of what a finding is about"
        );
    }

    #[test]
    fn the_details_of_a_finding_give_back_its_combination_and_other_details_give_none() {
        for channel in [None, Some("web")] {
            let c = combination(channel);
            let row = served_row(&rules(), &c, ContentHash::from_bytes([1; 32]), None);
            assert_eq!(combination_of(&row.details), Some(c));
        }
        assert_eq!(combination_of(&json!({})), None);
        assert_eq!(
            combination_of(&json!({"application": "core", "tenant": "sit1", "channel": 3, "file": "a"})),
            None
        );
        assert_eq!(
            combination_of(
                &json!({"application": "core", "tenant": "bad tenant", "channel": null, "file": "a"})
            ),
            None
        );
    }

    #[test]
    fn a_report_adds_up_and_counts_what_was_written() {
        let mut total = ServedReport::default();
        total.add(&ServedReport {
            swimlanes: 1,
            requests: 200,
            matched: 198,
            mismatched: 2,
            inserted: 2,
            ..ServedReport::default()
        });
        total.add(&ServedReport {
            offline: 1,
            resolved: 1,
            ..ServedReport::default()
        });
        assert_eq!((total.swimlanes, total.offline, total.requests), (1, 1, 200));
        assert_eq!(total.written(), 3);
    }
}
