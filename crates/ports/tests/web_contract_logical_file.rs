//! contract/08-web, `CG2`: `logical_file` and `tenant` on `FileEntry`, `Finding`, `SettingHit` and `TextHit`.
//!
//! Three things are held here. The document declares the two fields on the four schemas, optional, and its `required`
//! list is exactly the keys a value with neither field serialises to. The registry fake fills them from the path mapping
//! and the swimlane's tenants. And the registry conformance suite has teeth for them: an implementation that forgets the
//! fields, names the wrong tenant or gives a folder a logical file fails it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    CompareQuery, CompareRef, Comparison, ContentHash, ContentSource, DriftQuery, DriftState,
    EffectiveConfig, EffectiveQuery, FileClass, FileContent, FileEntry, FileKind, FileRole, Finding,
    FindingId, FindingKind, FindingsQuery, Grid, GridQuery, LineRange, LogicalFile, NfsPath, Page, Paged,
    Role, ServeRequest, ServedFile, ServiceState, SettingHit, SettingLocation, SettingPath, SettingValue,
    SettingsQuery, Severity, ShortText, SwimlaneId, SwimlaneSummary, TenantId, TextHit, TextQuery,
    TextResults, Timestamp, TreeComparison, User, ValueType, Version,
};
use ports::conformance::sample::{active, nfs, swimlane};
use ports::conformance::{self, RegistryScenario};
use ports::fakes::FakeRegistry;
use ports::{ReadError, RegistryRead};
use serde::Serialize;
use serde_json::Value;

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

/// The two fields of CG2, set the same way on each type.
trait Logical {
    fn set(&mut self, logical_file: Option<LogicalFile>, tenant: Option<TenantId>);
}

macro_rules! logical {
    ($($t:ty),*) => {$(
        impl Logical for $t {
            fn set(&mut self, logical_file: Option<LogicalFile>, tenant: Option<TenantId>) {
                self.logical_file = logical_file;
                self.tenant = tenant;
            }
        }
    )*};
}
logical!(FileEntry, Finding, SettingHit, TextHit);

fn lf(p: &str) -> LogicalFile {
    LogicalFile::parse(p).unwrap()
}

fn tid(t: &str) -> TenantId {
    TenantId::parse(t).unwrap()
}

fn file_entry(path: &str, is_dir: bool) -> FileEntry {
    FileEntry {
        path: nfs(path),
        is_dir,
        kind: FileKind::Base,
        class: FileClass::Structured,
        role: FileRole::PropertySource,
        size: 3,
        hash: (!is_dir).then(|| ContentHash::from_bytes([1; 32])),
        drift: DriftState::NfsAhead,
        format: None,
        content_withheld: false,
        attribution: None,
        severity: None,
        pickup_state: None,
        logical_file: None,
        tenant: None,
    }
}

fn finding(path: Option<&str>) -> Finding {
    Finding {
        id: FindingId::from(1),
        kind: FindingKind::Drift,
        swimlane: swimlane("sit1"),
        path: path.map(nfs),
        severity: Severity::High,
        rule_ids: Vec::new(),
        message: ShortText::parse("A plain sentence from the engine.").unwrap(),
        locations: Vec::new(),
        detected_at: Timestamp::from_unix_millis(1_700_000_000_000),
        logical_file: None,
        tenant: None,
    }
}

fn setting_hit(file: &str) -> SettingHit {
    SettingHit {
        swimlane: swimlane("sit1"),
        file: nfs(file),
        path: SettingPath::parse("a.b").unwrap(),
        value: SettingValue {
            text: "1".to_owned(),
            ty: ValueType::Int,
            flag: None,
        },
        location: SettingLocation {
            path: SettingPath::parse("a.b").unwrap(),
            lines: LineRange::new(1, 1).unwrap(),
        },
        logical_file: None,
        tenant: None,
    }
}

fn text_hit(path: &str) -> TextHit {
    TextHit {
        swimlane: swimlane("sit1"),
        path: nfs(path),
        line: 1,
        text: "a: 1".to_owned(),
        ranges: vec![(0, 1)],
        logical_file: None,
        tenant: None,
    }
}

fn json_of<T: Logical + Clone + Serialize>(base: &T, l: Option<LogicalFile>, t: Option<TenantId>) -> Value {
    let mut v = base.clone();
    v.set(l, t);
    serde_json::to_value(v).unwrap()
}

// ---------------------------------------------------------------------------------------------------------------
// The document

/// Both fields are on all four schemas, optional (an old client does not send them; an old mock stays valid), nullable, of
/// the types the domain serialises, and the schema stays closed. The `required` list is exactly the keys of a value with
/// neither field (they are skipped when `None`), and the declared properties are exactly the keys of a value with both.
#[test]
fn logical_file_additions_are_optional_and_match_the_domain() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let some = || (Some(lf("tx-core/app.yml")), Some(tid("sit1")));
    let samples: [(&str, Value, Value); 4] = {
        let (l, t) = some();
        [
            (
                "FileEntry",
                json_of(&file_entry("tx-core/app-sit1.yml", false), None, None),
                json_of(&file_entry("tx-core/app-sit1.yml", false), l.clone(), t.clone()),
            ),
            (
                "Finding",
                json_of(&finding(Some("tx-core/app-sit1.yml")), None, None),
                json_of(&finding(Some("tx-core/app-sit1.yml")), l.clone(), t.clone()),
            ),
            (
                "SettingHit",
                json_of(&setting_hit("tx-core/app-sit1.yml"), None, None),
                json_of(&setting_hit("tx-core/app-sit1.yml"), l.clone(), t.clone()),
            ),
            (
                "TextHit",
                json_of(&text_hit("tx-core/app-sit1.yml"), None, None),
                json_of(&text_hit("tx-core/app-sit1.yml"), l, t),
            ),
        ]
    };
    for (name, without, with) in &samples {
        let schema = &schemas[*name];
        assert_eq!(schema["additionalProperties"], false, "{name} stays closed");
        for (prop, target) in [("logical_file", "LogicalFile"), ("tenant", "TenantId")] {
            let def = &schema["properties"][prop];
            assert!(!def.is_null(), "{name}.{prop} is missing");
            assert_eq!(
                describe(def),
                format!("ref:{target}|null"),
                "{name}.{prop} is the domain type or null"
            );
            assert!(
                !required_of(schema).contains(prop),
                "{name}.{prop} must stay optional: an old client does not send it"
            );
            assert!(
                def["description"].as_str().is_some_and(|t| !t.is_empty()),
                "{name}.{prop} says what it is"
            );
            assert!(without.get(prop).is_none(), "{name}: `None` is not on the wire");
            assert!(with.get(prop).is_some(), "{name}: a value is on the wire");
        }
        assert_eq!(
            required_of(schema),
            wire_keys(without),
            "{name}: `required` is the keys of a value with neither field, so a new required key is a break"
        );
        assert_eq!(
            declared_of(schema),
            wire_keys(with),
            "{name}: the declared properties are the keys the domain writes"
        );
    }

    // The words that tell a client not to work it out itself.
    let text = schemas["FileEntry"]["properties"]["logical_file"]["description"]
        .as_str()
        .unwrap();
    assert!(text.contains("reverse mapping"), "{text}");
    assert!(text.contains("client never derives"), "{text}");
    let tenant = schemas["FileEntry"]["properties"]["tenant"]["description"]
        .as_str()
        .unwrap();
    assert!(tenant.contains("base file"), "{tenant}");
}

// ---------------------------------------------------------------------------------------------------------------
// The fake

fn bytes(s: &str) -> Bytes {
    Bytes::copy_from_slice(s.as_bytes())
}

fn seeded() -> FakeRegistry {
    let r = FakeRegistry::new();
    assert!(r.seed(
        &swimlane("sit1"),
        &[
            (nfs("tx-core.yml"), bytes("a: 1\n")),
            (nfs("tx-core-sit1.yml"), bytes("a: 2\n")),
            (nfs("tx-core-sit10.yml"), bytes("a: 3\n")),
            (nfs("svc/app-sit1.yml"), bytes("b: 1\n")),
        ]
    ));
    r
}

/// `(logical_file, tenant)` of an entry or hit, as the strings the wire would hold.
fn pair(l: Option<&LogicalFile>, t: Option<&TenantId>) -> (Option<String>, Option<String>) {
    (l.map(|x| x.as_str().to_owned()), t.map(|x| x.as_str().to_owned()))
}

fn expect(logical: Option<&str>, tenant: Option<&str>) -> (Option<String>, Option<String>) {
    (logical.map(str::to_owned), tenant.map(str::to_owned))
}

#[tokio::test]
async fn the_fake_names_the_logical_file_and_the_tenant_of_every_entry_it_lists() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    assert!(r.set_tenants(&s, vec![tid("sit1")]));
    let root = r
        .tree(&viewer, &s, &NfsPath::root(), Page::default())
        .await
        .unwrap();
    let by_path = |p: &str| {
        root.items
            .iter()
            .find(|e| e.path.as_str() == p)
            .unwrap_or_else(|| panic!("{p} is not listed"))
    };
    let of = |e: &FileEntry| pair(e.logical_file.as_ref(), e.tenant.as_ref());

    let base = by_path("tx-core.yml");
    assert_eq!(of(base), expect(Some("tx-core.yml"), None));
    assert_eq!(base.kind, FileKind::Base);
    let tenant_file = by_path("tx-core-sit1.yml");
    assert_eq!(of(tenant_file), expect(Some("tx-core.yml"), Some("sit1")));
    assert_eq!(
        tenant_file.kind,
        FileKind::Tenant,
        "a file with a tenant is a tenant file"
    );
    // `sit10` is not `sit1`: the suffix is exact, never a prefix match and never split on the first hyphen.
    assert_eq!(
        of(by_path("tx-core-sit10.yml")),
        expect(Some("tx-core-sit10.yml"), None)
    );
    // A folder has neither.
    assert_eq!(of(by_path("svc")), expect(None, None));
    let below = r.tree(&viewer, &s, &nfs("svc"), Page::default()).await.unwrap();
    assert_eq!(
        pair(
            below.items[0].logical_file.as_ref(),
            below.items[0].tenant.as_ref()
        ),
        expect(Some("svc/app.yml"), Some("sit1"))
    );

    // A swimlane the fake was not told the tenants of knows none: every file is a base file.
    let r2 = seeded();
    let all = r2
        .tree(&viewer, &s, &NfsPath::root(), Page::default())
        .await
        .unwrap();
    assert!(
        all.items.iter().filter(|e| !e.is_dir).all(|e| e.tenant.is_none()
            && e.logical_file.as_ref().map(LogicalFile::as_str) == Some(e.path.as_str())),
        "{all:?}"
    );
}

#[tokio::test]
async fn the_fake_lists_the_tenants_it_was_given_and_refuses_an_unknown_swimlane() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    let tenants_of = |list: Vec<SwimlaneSummary>| {
        list.into_iter()
            .find(|x| x.id == s)
            .unwrap()
            .tenants
            .iter()
            .map(|t| t.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(tenants_of(r.swimlanes(&viewer).await.unwrap()).is_empty());
    assert!(r.set_tenants(&s, vec![tid("sit1"), tid("sit2")]));
    assert_eq!(tenants_of(r.swimlanes(&viewer).await.unwrap()), ["sit1", "sit2"]);
    assert!(
        !r.set_tenants(&swimlane("nowhere"), vec![tid("sit1")]),
        "an unknown swimlane has no tenants to set"
    );
    // A new seed keeps the tenants: they are an attribute of the swimlane, not of its files.
    assert!(r.seed(&s, &[(nfs("x.yml"), bytes("x: 1\n"))]));
    assert_eq!(tenants_of(r.swimlanes(&viewer).await.unwrap()), ["sit1", "sit2"]);
}

/// A fake whose swimlane `sit1` has the tenant `sit1`, and the viewer that reads it.
fn tenanted() -> (FakeRegistry, SwimlaneId, User) {
    let (r, s) = (seeded(), swimlane("sit1"));
    assert!(r.set_tenants(&s, vec![tid("sit1")]));
    (r, s, active(1, Role::Viewer))
}

/// A scripted file with neither field gets them; one that says them keeps them; a folder never has them.
#[tokio::test]
async fn the_fake_fills_a_scripted_drift_list_and_keeps_what_the_script_says() {
    let (r, s, viewer) = tenanted();
    let mut said = file_entry("svc/app-sit1.yml", false);
    said.set(Some(lf("elsewhere.yml")), None);
    r.set_drift(
        &s,
        vec![
            file_entry("tx-core-sit1.yml", false),
            file_entry("tx-core.yml", false),
            said,
            file_entry("svc", true),
        ],
    );
    let drift = r
        .drift(
            &viewer,
            &s,
            &DriftQuery {
                state: None,
                prefix: None,
            },
            Page::default(),
        )
        .await
        .unwrap();
    let got: Vec<_> = drift
        .items
        .iter()
        .map(|e| {
            (
                e.path.as_str().to_owned(),
                pair(e.logical_file.as_ref(), e.tenant.as_ref()),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("svc/app-sit1.yml".to_owned(), expect(Some("elsewhere.yml"), None)),
            (
                "tx-core-sit1.yml".to_owned(),
                expect(Some("tx-core.yml"), Some("sit1"))
            ),
            ("tx-core.yml".to_owned(), expect(Some("tx-core.yml"), None)),
        ]
    );
}

/// The swimlane of the hit says which tenants apply.
#[tokio::test]
async fn the_fake_fills_scripted_setting_hits() {
    let (r, _, viewer) = tenanted();
    r.set_setting_hits(vec![setting_hit("tx-core-sit1.yml"), setting_hit("tx-core.yml")]);
    let settings = r
        .search_settings(
            &viewer,
            SettingsQuery {
                path_contains: ShortText::parse("a").unwrap(),
                swimlanes: Vec::new(),
                file: None,
                page: Page::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        settings
            .items
            .iter()
            .map(|h| pair(h.logical_file.as_ref(), h.tenant.as_ref()))
            .collect::<Vec<_>>(),
        [
            expect(Some("tx-core.yml"), Some("sit1")),
            expect(Some("tx-core.yml"), None)
        ]
    );
}

#[tokio::test]
async fn the_fake_fills_scripted_text_hits() {
    let (r, _, viewer) = tenanted();
    r.set_text_results(TextResults {
        hits: vec![text_hit("tx-core-sit1.yml")],
        truncated: false,
    });
    let text = r
        .search_text(
            &viewer,
            TextQuery {
                pattern: "a".to_owned(),
                regex: false,
                case_sensitive: true,
                swimlanes: Vec::new(),
                path_glob: None,
                max_total: TextQuery::DEFAULT_MAX_TOTAL,
                max_per_file: TextQuery::DEFAULT_MAX_PER_FILE,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        pair(text.hits[0].logical_file.as_ref(), text.hits[0].tenant.as_ref()),
        expect(Some("tx-core.yml"), Some("sit1"))
    );
}

/// A finding with a path gets them, one with no path has neither.
#[tokio::test]
async fn the_fake_fills_findings_that_name_a_path() {
    let (r, _, viewer) = tenanted();
    r.add_finding(finding(Some("tx-core-sit1.yml")));
    r.add_finding(finding(None));
    let findings = r
        .findings(
            &viewer,
            FindingsQuery {
                swimlane: None,
                kinds: Vec::new(),
                min_severity: None,
                page: Page::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        findings
            .items
            .iter()
            .map(|f| pair(f.logical_file.as_ref(), f.tenant.as_ref()))
            .collect::<Vec<_>>(),
        [expect(Some("tx-core.yml"), Some("sit1")), expect(None, None)]
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The conformance suite has teeth

/// How a [`Wrapper`] gets `logical_file` and `tenant` wrong. `Honest` gets them right.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deviation {
    Honest,
    /// Cannot be told a swimlane's tenants (the default of the harness), and is honest otherwise.
    CannotNameTenants,
    /// Forgets both fields, as an implementation written before the change does.
    ForgetsBoth,
    /// Forgets the logical file of the files it lists.
    ForgetsTheLogicalFile,
    /// Gives a folder a logical file.
    FolderHasALogicalFile,
    /// Says a base file belongs to a tenant.
    InventsATenant,
    /// Reports a tenant file as a base file, consistently: its own name as the logical file and no tenant. Only the suite's
    /// knowledge of the swimlane's tenants can tell.
    ReportsATenantFileAsBase,
    /// Keeps the tenant but reports the file's own name as its logical file.
    LogicalFileKeepsTheSuffix,
    /// Lists the tenants of a swimlane as none.
    SwimlanesForgetTheirTenants,
    /// Lists a tenant file in the drift list with no logical file.
    DriftForgetsTheLogicalFile,
}

/// Forwards every method to a fake and deviates in one.
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

    fn damage(&self, e: &mut FileEntry) {
        match self.how {
            Deviation::ForgetsBoth => e.set(None, None),
            Deviation::ForgetsTheLogicalFile if !e.is_dir => e.logical_file = None,
            Deviation::FolderHasALogicalFile if e.is_dir => e.logical_file = Some(lf("a.yml")),
            Deviation::InventsATenant if !e.is_dir && e.tenant.is_none() => e.tenant = Some(tid("sit1")),
            Deviation::ReportsATenantFileAsBase if e.tenant.is_some() => {
                e.logical_file = Some(lf(e.path.as_str()));
                e.tenant = None;
            }
            Deviation::LogicalFileKeepsTheSuffix if e.tenant.is_some() => {
                e.logical_file = Some(lf(e.path.as_str()));
            }
            _ => {}
        }
    }
}

#[async_trait]
impl RegistryRead for Wrapper {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        let mut list = self.inner.swimlanes(u).await?;
        if self.how == Deviation::SwimlanesForgetTheirTenants {
            for s in &mut list {
                s.tenants.clear();
            }
        }
        Ok(list)
    }
    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        let mut p = self.inner.tree(u, s, prefix, page).await?;
        p.items.iter_mut().for_each(|e| self.damage(e));
        Ok(p)
    }
    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError> {
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
        let honest = self.inner.drift(u, s, q, page).await?;
        if self.how == Deviation::DriftForgetsTheLogicalFile {
            // One file the suite did not seed, in a state that is drift, with a tenant and no logical file.
            let mut e = file_entry("a/one-sit1.yml", false);
            e.tenant = Some(tid("sit1"));
            return Ok(Paged::last(vec![e]));
        }
        Ok(honest)
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
        // A harness that cannot name tenants keeps the trait's default and answers `false`.
        self.how != Deviation::CannotNameTenants
            && FakeRegistry::set_tenants(&self.inner, s, tenants.to_vec())
    }
}

#[tokio::test(start_paused = true)]
async fn the_registry_suite_passes_a_registry_that_names_the_fields_right() {
    conformance::registry_read(&Wrapper::new(Deviation::Honest)).await;
    // The suite can only name the tenants of a swimlane when the harness can, and passes the generic rules when it cannot.
    conformance::registry_read(&Wrapper::new(Deviation::CannotNameTenants)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "names its logical file")]
async fn a_registry_that_forgets_both_fields_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsBoth)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "names its logical file")]
async fn a_registry_that_forgets_the_logical_file_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ForgetsTheLogicalFile)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a directory has no logical file and no tenant")]
async fn a_registry_that_gives_a_folder_a_logical_file_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::FolderHasALogicalFile)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "names a tenant that its name does not end with")]
async fn a_registry_that_invents_a_tenant_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::InventsATenant)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "tx-core-sit1.yml belongs to the tenant sit1")]
async fn a_registry_that_reports_a_tenant_file_as_a_base_file_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::ReportsATenantFileAsBase)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "the logical file of a tenant file is its name without the suffix")]
async fn a_registry_whose_logical_file_keeps_the_tenant_suffix_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::LogicalFileKeepsTheSuffix)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "lists the tenants it was given")]
async fn a_registry_whose_swimlane_list_forgets_the_tenants_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::SwimlanesForgetTheirTenants)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "names its logical file")]
async fn a_registry_whose_drift_list_forgets_the_logical_file_fails() {
    conformance::registry_read(&Wrapper::new(Deviation::DriftForgetsTheLogicalFile)).await;
}
