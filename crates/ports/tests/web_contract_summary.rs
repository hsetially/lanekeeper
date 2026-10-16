//! contract/08-web, `CG4a`: `tier`, `project`, `baseline`, `pickup_counts` and `syncing` on `SwimlaneSummary`, and `UserRef`.
//!
//! The document declares the five fields optional (an old client, an old mock and an old server stay valid), the new schemas
//! are the domain's, and `UserRef` has no email (Q6). The registry fake lists what it is told and derives the pickup counts
//! from the pending restarts. And the registry conformance suite has teeth for the fields: an implementation that forgets
//! the baseline, counts pickup states that disagree with the pending restarts, or drops what it was given fails it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    BaselineState, BaselineStatus, CompareQuery, CompareRef, Comparison, ContentSource, DriftQuery,
    EffectiveConfig, EffectiveQuery, FileEntry, Finding, FindingsQuery, Grid, GridQuery, JobRef, LogicalFile,
    NfsPath, Page, Paged, PickupState, Role, ServeRequest, ServedFile, ServiceRef, ServiceState, SettingHit,
    SettingsQuery, ShortText, SwimlaneId, SwimlaneSummary, SyncingInfo, TenantId, TextQuery, TextResults,
    Timestamp, TreeComparison, User, UserRef, UserStatus, Version,
};
use ports::conformance::sample::{active, nfs, swimlane, user};
use ports::conformance::{self, RegistryScenario};
use ports::fakes::FakeRegistry;
use ports::{ReadError, RegistryRead};
use serde_json::{Value, json};

const SPEC: &str = include_str!("../../../api/openapi.yaml");
const SCHEMA_REF: &str = "#/components/schemas/";

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

fn ref_name(v: &Value) -> &str {
    let r = v["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a $ref, got {v}"));
    r.strip_prefix(SCHEMA_REF)
        .unwrap_or_else(|| panic!("not a schema $ref: {r}"))
}

fn names(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema.get("required").map(names).unwrap_or_default()
}

fn declared_of(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {schema}"))
        .keys()
        .cloned()
        .collect()
}

fn wire_keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

/// A property schema as one short string, so that a table can say what the document must hold.
fn describe(s: &Value) -> String {
    if s.get("$ref").is_some() {
        return format!("ref:{}", ref_name(s));
    }
    if let Some(arms) = s.get("oneOf").and_then(Value::as_array) {
        return arms.iter().map(describe).collect::<Vec<_>>().join("|");
    }
    match s["type"].as_str() {
        Some(other) => other.to_owned(),
        None => panic!("a property with neither $ref, oneOf nor type: {s}"),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Values

fn short(t: &str) -> ShortText {
    ShortText::parse(t).unwrap()
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(ms)
}

fn job() -> JobRef {
    JobRef::new("sync-job-1", "uid-1").unwrap()
}

fn confirmed_by_someone() -> BaselineStatus {
    BaselineStatus {
        state: BaselineState::Confirmed,
        by: Some(UserRef::from(&active(1, Role::Admin))),
        at: Some(at(1_700_000_000_000)),
    }
}

fn in_progress() -> BaselineStatus {
    BaselineStatus {
        state: BaselineState::InProgress,
        by: None,
        at: None,
    }
}

fn syncing() -> SyncingInfo {
    SyncingInfo {
        job: job(),
        since: at(1_700_000_001_000),
    }
}

/// A summary the way a producer wrote it before the change.
fn old_summary() -> SwimlaneSummary {
    serde_json::from_value(json!({
        "id": "sit1", "display_name": "SIT 1", "tenants": ["sit1"], "agent": "connected", "last_scan_at": null,
        "base_version": null, "drift_counts": [["nfs_ahead", 2]], "finding_count": 4
    }))
    .unwrap()
}

fn full_summary() -> SwimlaneSummary {
    SwimlaneSummary {
        tier: Some(short("SIT")),
        project: Some(short("Core Banking")),
        baseline: Some(confirmed_by_someone()),
        pickup_counts: vec![(PickupState::NeedsNotifyOrRestart, 2)],
        syncing: Some(syncing()),
        ..old_summary()
    }
}

fn service(name: &str, pickup: PickupState) -> ServiceState {
    ServiceState {
        service: ServiceRef::new("csp", name).unwrap(),
        pickup,
        pending_since: None,
        live_by: None,
        notifications_enabled: true,
        cache_ttl_secs: 1200,
        pending_files: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The document

/// The five fields are on `SwimlaneSummary`, none required, of the types the domain writes, and the schema stays closed. The
/// `required` list is exactly the keys of a summary with none of them, and the declared properties are exactly the keys of
/// one with all of them.
#[test]
fn swimlane_summary_additions_are_optional_and_match_the_domain() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let summary = &schemas["SwimlaneSummary"];
    assert_eq!(summary["additionalProperties"], false);
    let without = serde_json::to_value(old_summary()).unwrap();
    let with = serde_json::to_value(full_summary()).unwrap();

    for (prop, kind) in [
        ("tier", "ref:ShortText|null"),
        ("project", "ref:ShortText|null"),
        ("baseline", "ref:BaselineStatus|null"),
        ("syncing", "ref:SyncingInfo|null"),
        ("pickup_counts", "array"),
    ] {
        let def = &summary["properties"][prop];
        assert!(!def.is_null(), "SwimlaneSummary.{prop} is missing");
        assert_eq!(describe(def), kind, "SwimlaneSummary.{prop}");
        assert!(
            !required_of(summary).contains(prop),
            "SwimlaneSummary.{prop} must stay optional: an old client does not send it"
        );
        assert!(without.get(prop).is_none(), "{prop}: absent is not on the wire");
        assert!(with.get(prop).is_some(), "{prop}: a value is on the wire");
        assert!(
            def["description"].as_str().is_some_and(|t| !t.is_empty()),
            "SwimlaneSummary.{prop} says what it is"
        );
    }
    assert_eq!(
        required_of(summary),
        wire_keys(&without),
        "`required` is the keys of an old summary, so a new required key is a break"
    );
    assert_eq!(declared_of(summary), wire_keys(&with));
    // `SwimlaneDetail` inherits the summary, so it has them too.
    assert_eq!(
        ref_name(&schemas["SwimlaneDetail"]["allOf"][0]),
        "SwimlaneSummary"
    );

    // The pickup counts are pairs, like `drift_counts`: a state and a count, at most one pair per non-live state.
    let counts = &summary["properties"]["pickup_counts"];
    assert_eq!(counts["maxItems"], PickupState::ALL.len() as u64 - 1);
    let pair = &counts["items"];
    assert_eq!(pair["type"], "array");
    assert_eq!(
        (pair["minItems"].clone(), pair["maxItems"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(describe(&pair["prefixItems"][0]), "ref:PickupState");
    assert_eq!(pair["prefixItems"][1]["type"], "integer");
    assert_eq!(pair["prefixItems"][1]["minimum"], 0);
    assert_eq!(with["pickup_counts"], json!([["needs_notify_or_restart", 2]]));
    let text = counts["description"].as_str().unwrap();
    assert!(text.contains("listPendingRestarts"), "{text}");
    assert!(text.contains("aggregate"), "{text}");
    assert!(
        summary["properties"]["baseline"]["description"]
            .as_str()
            .unwrap()
            .contains("open adoption"),
        "the baseline says what `in_progress` is"
    );
}

#[test]
fn baseline_syncing_and_user_ref_schemas_are_the_domains() {
    let d = doc();
    let schemas = &d["components"]["schemas"];

    let states: BTreeSet<String> = BaselineState::ALL.iter().map(|s| s.as_str().to_owned()).collect();
    assert_eq!(names(&schemas["BaselineState"]["enum"]), states);

    // A new schema follows the document's rule: every field present, a nullable one `null`.
    let closed = |name: &str, with: &Value, extra: &[&Value]| {
        let schema = &schemas[name];
        assert_eq!(schema["additionalProperties"], false, "{name} is closed");
        assert_eq!(
            required_of(schema),
            wire_keys(with),
            "{name}: required is what is written"
        );
        assert_eq!(
            declared_of(schema),
            wire_keys(with),
            "{name}: declared is what is written"
        );
        for other in extra {
            assert_eq!(
                required_of(schema),
                wire_keys(other),
                "{name}: whatever the values are"
            );
        }
        assert!(
            schema["description"].as_str().is_some_and(|t| !t.is_empty()),
            "{name} says what it is"
        );
    };
    let baseline_open = serde_json::to_value(in_progress()).unwrap();
    let baseline_done = serde_json::to_value(confirmed_by_someone()).unwrap();
    closed("BaselineStatus", &baseline_done, &[&baseline_open]);
    let baseline_props = &schemas["BaselineStatus"]["properties"];
    assert_eq!(describe(&baseline_props["state"]), "ref:BaselineState");
    assert_eq!(describe(&baseline_props["by"]), "ref:UserRef|null");
    assert_eq!(describe(&baseline_props["at"]), "ref:Timestamp|null");

    closed("SyncingInfo", &serde_json::to_value(syncing()).unwrap(), &[]);
    let info = &schemas["SyncingInfo"]["properties"];
    assert_eq!(describe(&info["job"]), "ref:JobRef");
    assert_eq!(describe(&info["since"]), "ref:Timestamp");

    closed("UserRef", &baseline_done["by"], &[]);
    let user_ref = &schemas["UserRef"]["properties"];
    assert_eq!(describe(&user_ref["id"]), "ref:UserId");
    assert_eq!(describe(&user_ref["display_name"]), "ref:ShortText");
    // Q6: a name every signed-in user may see, never an address.
    for schema in ["UserRef", "BaselineStatus", "SyncingInfo"] {
        for prop in declared_of(&schemas[schema]) {
            assert!(
                !prop.contains("mail"),
                "{schema}.{prop}: no email in a type every user can read"
            );
        }
    }
    assert!(
        schemas["UserRef"]["description"]
            .as_str()
            .unwrap()
            .contains("email"),
        "UserRef says it holds no email"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The fake

fn bytes(s: &str) -> Bytes {
    Bytes::copy_from_slice(s.as_bytes())
}

fn seeded() -> FakeRegistry {
    let r = FakeRegistry::new();
    assert!(r.seed(&swimlane("sit1"), &[(nfs("a.yml"), bytes("a: 1\n"))]));
    r
}

async fn summary_of(r: &impl RegistryRead, s: &SwimlaneId) -> SwimlaneSummary {
    r.swimlanes(&active(1, Role::Viewer))
        .await
        .unwrap()
        .into_iter()
        .find(|x| &x.id == s)
        .expect("the swimlane is listed")
}

#[tokio::test]
async fn the_fake_says_unconfirmed_and_nothing_else_until_it_is_told() {
    let (r, s) = (seeded(), swimlane("sit1"));
    let got = summary_of(&r, &s).await;
    assert_eq!(
        got.baseline,
        Some(BaselineStatus {
            state: BaselineState::Unconfirmed,
            by: None,
            at: None
        }),
        "a swimlane nobody adopted has an unconfirmed baseline, and it says so"
    );
    assert_eq!((got.tier, got.project, got.syncing), (None, None, None));
    assert!(got.pickup_counts.is_empty());
}

#[tokio::test]
async fn the_fake_lists_what_it_is_told_and_keeps_it_across_a_seed() {
    let (r, s) = (seeded(), swimlane("sit1"));
    assert!(r.set_tier_and_project(&s, Some(short("SIT")), Some(short("Core Banking"))));
    assert!(r.set_baseline(&s, confirmed_by_someone()));
    assert!(r.set_syncing(&s, Some(syncing())));
    let got = summary_of(&r, &s).await;
    assert_eq!(got.tier, Some(short("SIT")));
    assert_eq!(got.project, Some(short("Core Banking")));
    assert_eq!(got.baseline, Some(confirmed_by_someone()));
    assert_eq!(got.syncing, Some(syncing()));

    // They are attributes of the swimlane, so a new seed of its files keeps them; the window can be closed.
    assert!(r.seed(&s, &[(nfs("b.yml"), bytes("b: 1\n"))]));
    assert_eq!(summary_of(&r, &s).await.tier, Some(short("SIT")));
    assert!(r.set_syncing(&s, None));
    assert_eq!(summary_of(&r, &s).await.syncing, None);

    // An unknown swimlane has nothing to set.
    let nowhere = swimlane("nowhere");
    assert!(!r.set_tier_and_project(&nowhere, None, None));
    assert!(!r.set_baseline(&nowhere, in_progress()));
    assert!(!r.set_syncing(&nowhere, None));
}

/// The counts are worked out from the pending restarts the fake holds, so they agree with `pending_restarts` by
/// construction: the non-live states, in the order of `PickupState::ALL`, none with a count of zero.
#[tokio::test]
async fn the_fake_derives_the_pickup_counts_from_the_pending_restarts() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    r.set_pending_restarts(
        &s,
        vec![
            service("tx-api", PickupState::NeedsNotifyOrRestart),
            service("tx-batch", PickupState::NeedsConfigServerRestart),
            service("tx-ui", PickupState::NeedsNotifyOrRestart),
            service("tx-docs", PickupState::LiveWithinTtl),
            service("tx-live", PickupState::Live),
        ],
    );
    assert_eq!(
        summary_of(&r, &s).await.pickup_counts,
        [
            (PickupState::LiveWithinTtl, 1),
            (PickupState::NeedsNotifyOrRestart, 2),
            (PickupState::NeedsConfigServerRestart, 1),
        ]
    );
    assert_eq!(
        r.pending_restarts(&viewer, &s).await.unwrap().len(),
        5,
        "`pending_restarts` lists every service, live ones included; the counts leave the live ones out"
    );
    r.set_pending_restarts(&s, Vec::new());
    assert!(summary_of(&r, &s).await.pickup_counts.is_empty());
}

// ---------------------------------------------------------------------------------------------------------------
// The conformance suite has teeth

/// How a [`Wrapper`] gets the summary additions wrong. `Honest` gets them right.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deviation {
    Honest,
    /// Cannot be told a swimlane's tier, baseline, window or pending restarts (the harness default), and is honest otherwise.
    CannotScript,
    ForgetsTheBaseline,
    /// Says an unconfirmed baseline was confirmed by someone.
    UnconfirmedNamesSomeone,
    /// Says a baseline is confirmed and not when.
    ConfirmedWithoutWhen,
    /// Always says the baseline is unconfirmed, whatever it was given.
    ForgetsTheBaselineItWasGiven,
    /// Counts a state nobody is in.
    PickupCountsDisagree,
    PickupCountsIncludeLive,
    /// Lists no counts at all.
    ForgetsThePickupCounts,
    ForgetsTierAndProject,
    ForgetsSyncing,
}

/// Forwards every method to a fake, and deviates in the summaries `swimlanes` lists.
struct Wrapper {
    inner: FakeRegistry,
    how: Deviation,
}

impl Wrapper {
    fn new(how: Deviation) -> Self {
        Self {
            inner: FakeRegistry::new(),
            how,
        }
    }

    fn can_script(&self) -> bool {
        self.how != Deviation::CannotScript
    }

    fn damage(&self, s: &mut SwimlaneSummary) {
        match self.how {
            Deviation::ForgetsTheBaseline => s.baseline = None,
            Deviation::UnconfirmedNamesSomeone => {
                if let Some(b) = s
                    .baseline
                    .as_mut()
                    .filter(|b| b.state != BaselineState::Confirmed)
                {
                    b.by = confirmed_by_someone().by;
                }
            }
            Deviation::ConfirmedWithoutWhen => {
                if let Some(b) = s
                    .baseline
                    .as_mut()
                    .filter(|b| b.state == BaselineState::Confirmed)
                {
                    b.at = None;
                }
            }
            Deviation::ForgetsTheBaselineItWasGiven => {
                s.baseline = Some(BaselineStatus {
                    state: BaselineState::Unconfirmed,
                    by: None,
                    at: None,
                });
            }
            Deviation::PickupCountsDisagree => {
                s.pickup_counts.push((PickupState::NeedsConfigServerRestart, 99));
            }
            Deviation::PickupCountsIncludeLive => s.pickup_counts.push((PickupState::Live, 1)),
            Deviation::ForgetsThePickupCounts => s.pickup_counts.clear(),
            Deviation::ForgetsTierAndProject => {
                s.tier = None;
                s.project = None;
            }
            Deviation::ForgetsSyncing => s.syncing = None,
            Deviation::Honest | Deviation::CannotScript => {}
        }
    }
}

#[async_trait]
impl RegistryRead for Wrapper {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        let mut list = self.inner.swimlanes(u).await?;
        list.iter_mut().for_each(|s| self.damage(s));
        Ok(list)
    }
    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.inner.tree(u, s, prefix, page).await
    }
    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<domain::FileContent, ReadError> {
        self.inner.content(u, s, p, src).await
    }
    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        self.inner.history(u, s, p, page).await
    }
    async fn compare(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        self.inner.compare(u, l, r, p).await
    }
    async fn compare_tree(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.inner.compare_tree(u, l, r, page).await
    }
    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        self.inner.grid(u, q).await
    }
    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        self.inner.search_settings(u, q).await
    }
    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
        self.inner.search_text(u, q).await
    }
    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        self.inner.findings(u, q).await
    }
    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        self.inner.effective(u, s, f).await
    }
    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        self.inner.pending_restarts(u, s).await
    }
    async fn effective_for(
        &self,
        u: &User,
        s: &SwimlaneId,
        q: &EffectiveQuery,
    ) -> Result<EffectiveConfig, ReadError> {
        self.inner.effective_for(u, s, q).await
    }
    async fn compare_in(&self, u: &User, q: &CompareQuery) -> Result<Comparison, ReadError> {
        self.inner.compare_in(u, q).await
    }
    async fn compare_tree_under(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.inner.compare_tree_under(u, l, r, prefix, page).await
    }
    async fn drift(
        &self,
        u: &User,
        s: &SwimlaneId,
        q: &DriftQuery,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.inner.drift(u, s, q, page).await
    }
    async fn auto_notify(&self, u: &User, s: &SwimlaneId) -> Result<bool, ReadError> {
        self.inner.auto_notify(u, s).await
    }
    async fn served(&self, u: &User, s: &SwimlaneId, r: &ServeRequest) -> Result<ServedFile, ReadError> {
        self.inner.served(u, s, r).await
    }
}

#[async_trait]
impl RegistryScenario for Wrapper {
    async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
        FakeRegistry::seed(&self.inner, s, files);
    }
    async fn set_tenants(&self, s: &SwimlaneId, tenants: &[TenantId]) -> bool {
        FakeRegistry::set_tenants(&self.inner, s, tenants.to_vec())
    }
    // A harness that cannot script these keeps the trait's default and answers `false`.
    async fn set_pending_restarts(&self, s: &SwimlaneId, services: Vec<ServiceState>) -> bool {
        self.can_script() && {
            FakeRegistry::set_pending_restarts(&self.inner, s, services);
            true
        }
    }
    async fn set_tier_and_project(
        &self,
        s: &SwimlaneId,
        tier: Option<ShortText>,
        project: Option<ShortText>,
    ) -> bool {
        self.can_script() && FakeRegistry::set_tier_and_project(&self.inner, s, tier, project)
    }
    async fn set_baseline(&self, s: &SwimlaneId, baseline: BaselineStatus) -> bool {
        self.can_script() && FakeRegistry::set_baseline(&self.inner, s, baseline)
    }
    async fn set_syncing(&self, s: &SwimlaneId, syncing: Option<SyncingInfo>) -> bool {
        self.can_script() && FakeRegistry::set_syncing(&self.inner, s, syncing)
    }
}

#[tokio::test(start_paused = true)]
async fn the_registry_suite_passes_a_registry_that_gets_the_additions_right() {
    conformance::registry_read(&Wrapper::new(Deviation::Honest)).await;
    // When the harness cannot script a swimlane, the suite holds the registry to what is true of every answer.
    conformance::registry_read(&Wrapper::new(Deviation::CannotScript)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "swimlanes states the baseline of every swimlane")]
async fn a_registry_that_forgets_the_baseline_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsTheBaseline)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a baseline that is not confirmed names nobody and no time")]
async fn a_registry_whose_open_baseline_names_someone_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::UnconfirmedNamesSomeone)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a confirmed baseline says when it was confirmed")]
async fn a_registry_whose_confirmed_baseline_has_no_time_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ConfirmedWithoutWhen)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "swimlanes lists the baseline it was given")]
async fn a_registry_that_drops_the_baseline_it_was_given_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsTheBaselineItWasGiven)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "pickup_counts equal the pending restarts per state")]
async fn a_registry_whose_pickup_counts_disagree_with_the_pending_restarts_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::PickupCountsDisagree)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "pickup_counts lists no live state")]
async fn a_registry_that_counts_the_live_state_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::PickupCountsIncludeLive)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "pickup_counts equal the pending restarts per state")]
async fn a_registry_that_lists_no_counts_for_pending_restarts_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsThePickupCounts)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "swimlanes lists the tier and project it was given")]
async fn a_registry_that_forgets_the_tier_and_project_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsTierAndProject)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "swimlanes lists the sync window it was given")]
async fn a_registry_that_forgets_the_sync_window_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsSyncing)).await;
}

/// A user that is not a viewer of anything gets no summaries at all (S4): the additions do not open a side door.
#[tokio::test]
async fn the_additions_are_not_visible_to_a_user_who_may_not_read() {
    let (r, s) = (seeded(), swimlane("sit1"));
    assert!(r.set_baseline(&s, confirmed_by_someone()));
    for bad in [
        user(2, Some(Role::Admin), UserStatus::Pending),
        user(3, Some(Role::Admin), UserStatus::Disabled),
        user(4, None, UserStatus::Active),
    ] {
        assert_eq!(r.swimlanes(&bad).await.unwrap_err(), ReadError::Forbidden);
    }
}
