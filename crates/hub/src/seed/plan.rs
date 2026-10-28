//! What a seed run would change (plan 18, T7, G24; rule 8, S9): the file set against what the database holds, as a list of changes.
//!
//! **Pure.** [`plan`] takes the validated file and a [`State`] that the caller read from the database, and returns a [`Plan`]: no
//! I/O, so every rule below is a unit test. [`crate::seed::apply`] reads the state, calls this, and writes.
//!
//! **The rules** (the human's, for a database whose swimlanes are already running):
//!
//! - **An unchanged row is no change.** A row that holds what the file says is counted and not written, and writes no audit event.
//!   A service account that differs only in case is the same account (Google compares them without regard to case).
//! - **A field the file does not carry is never touched.** The change lists the fields the file carries *and* that differ.
//! - **Nothing is deleted.** There is no delete in [`Op`]. A row, a tenant or a sentinel that the database has and the file does not
//!   list is only *reported* ([`Absent`]).
//! - **One change is one audit event:** a project, a swimlane, the tenants added to a swimlane (the tenant set), a sentinel.
//!   [`Change::event`] makes it; [`MAX_CHANGES`] bounds a run so that the audit chain's lock is held briefly.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use domain::{ShortText, SwimlaneId, Timestamp};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia};
use serde::Serialize;

use super::error::{FieldPath, MAX_CHANGES, SeedError};
use super::file::{SeedFile, Swimlane};

/// The component name in the audit trail: the actor of every event a seed writes.
pub const COMPONENT: &str = "hub-seed";

/// How many names of each kind the report lists as "in the database and not in the file".
pub const REPORT_LIMIT: usize = 50;

// ------------------------------------------------------------------------------------------------ what the database holds

/// A swimlane as the database has it, for the columns the seed can write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SwimlaneRow {
    pub project: String,
    pub display_name: String,
    pub cluster: Option<String>,
    pub tier: Option<String>,
    pub nfs_server: Option<String>,
    pub nfs_export: Option<String>,
    pub mount_root: Option<String>,
    pub agent_service_account: Option<String>,
}

/// A sentinel as the database has it, for the columns the seed can write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SentinelRow {
    pub swimlane: Option<String>,
    pub export_root: String,
    pub service_account: Option<String>,
}

/// Rows the database has and the file does not list, for sections the file carries. Never changed; reported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Absent {
    pub projects: Vec<String>,
    pub swimlanes: Vec<String>,
    pub sentinels: Vec<String>,
    /// `(swimlane, tenant)`: tenants of a swimlane whose tenants the file lists, that the file does not.
    pub tenants: Vec<(String, String)>,
    /// A list was cut at [`REPORT_LIMIT`].
    pub truncated: bool,
}

impl Absent {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
            && self.swimlanes.is_empty()
            && self.sentinels.is_empty()
            && self.tenants.is_empty()
    }
}

/// What the database holds that the file speaks of: the rows the file names, the projects and swimlanes it refers to, and the
/// tenants of the swimlanes it names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// Project id to display name.
    pub projects: BTreeMap<String, String>,
    pub swimlanes: BTreeMap<String, SwimlaneRow>,
    /// Swimlane id to its tenants.
    pub tenants: BTreeMap<String, BTreeSet<String>>,
    /// Sentinel vm to its row.
    pub sentinels: BTreeMap<String, SentinelRow>,
    /// The rows of the sections the file carries that the file does not list (projects, swimlanes, sentinels).
    pub absent: Absent,
}

impl State {
    /// What the database would hold once `plan` had been applied to it: the rows the plan creates or updates, with the columns it sets,
    /// and the tenants it adds; nothing else moves. A run that read this state would find nothing to do, which is what "a second run
    /// changes nothing" means, and the fuzz target `hub_seed_file` and the tests hold the planner to it for any file.
    #[must_use]
    pub fn after(&self, plan: &Plan) -> Self {
        let mut next = self.clone();
        for change in &plan.changes {
            match change.kind {
                Kind::Project => {
                    if let Some(name) = change.value(Field::DisplayName) {
                        next.projects.insert(change.key.clone(), name.to_owned());
                    }
                }
                Kind::Swimlane => {
                    let row = next.swimlanes.entry(change.key.clone()).or_default();
                    for set in &change.fields {
                        let to = set.to.clone();
                        match set.field {
                            Field::Project => row.project = to,
                            Field::DisplayName => row.display_name = to,
                            Field::Cluster => row.cluster = Some(to),
                            Field::Tier => row.tier = Some(to),
                            Field::NfsServer => row.nfs_server = Some(to),
                            Field::NfsExport => row.nfs_export = Some(to),
                            Field::MountRoot => row.mount_root = Some(to),
                            Field::AgentServiceAccount => row.agent_service_account = Some(to),
                            Field::Swimlane | Field::ExportRoot | Field::ServiceAccount => {}
                        }
                    }
                }
                Kind::Tenants => {
                    next.tenants
                        .entry(change.key.clone())
                        .or_default()
                        .extend(change.tenants.iter().cloned());
                }
                Kind::Sentinel => {
                    let row = next.sentinels.entry(change.key.clone()).or_default();
                    for set in &change.fields {
                        let to = set.to.clone();
                        match set.field {
                            Field::Swimlane => row.swimlane = Some(to),
                            Field::ExportRoot => row.export_root = to,
                            Field::ServiceAccount => row.service_account = Some(to),
                            _ => {}
                        }
                    }
                }
            }
        }
        next
    }
}

// ------------------------------------------------------------------------------------------------ what would change

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Project,
    Swimlane,
    /// The tenants of one swimlane, as a set.
    Tenants,
    Sentinel,
}

impl Kind {
    /// The word in the report and in the audit `diff`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Swimlane => "swimlane",
            Self::Tenants => "swimlane_tenants",
            Self::Sentinel => "sentinel",
        }
    }
}

/// What is done to a row. There is no delete: a seed never removes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Create,
    Update,
    /// Tenants added to a swimlane's set.
    Add,
}

impl Op {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Add => "add",
        }
    }
}

/// A column the seed can write, by the name the file and the report use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Project,
    DisplayName,
    Cluster,
    Tier,
    NfsServer,
    NfsExport,
    MountRoot,
    AgentServiceAccount,
    Swimlane,
    ExportRoot,
    ServiceAccount,
}

impl Field {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::DisplayName => "display_name",
            Self::Cluster => "cluster",
            Self::Tier => "tier",
            Self::NfsServer => "nfs_server",
            Self::NfsExport => "nfs_export",
            Self::MountRoot => "mount_root",
            Self::AgentServiceAccount => "agent_service_account",
            Self::Swimlane => "swimlane",
            Self::ExportRoot => "export_root",
            Self::ServiceAccount => "service_account",
        }
    }
}

/// One column that a change sets: from what the database had (nothing, for a new row or an empty column) to what the file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub field: Field,
    pub from: Option<String>,
    pub to: String,
}

/// One row (or one tenant set) that would be created or updated: one audit event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: Kind,
    pub op: Op,
    /// The id of the project or swimlane, the swimlane of a tenant set, or the vm of a sentinel.
    pub key: String,
    /// The swimlane the event is filed under, where the change has one.
    pub swimlane: Option<SwimlaneId>,
    /// The columns set, for a create or an update.
    pub fields: Vec<FieldChange>,
    /// The tenants added, for [`Kind::Tenants`].
    pub tenants: Vec<String>,
}

impl Change {
    /// The value this change sets for `field`, if it sets it. The writer binds this and nothing else, so a column the change does not
    /// set is not in the statement's `SET` (it is `COALESCE`d to itself).
    #[must_use]
    pub fn value(&self, field: Field) -> Option<&str> {
        self.fields
            .iter()
            .find(|c| c.field == field)
            .map(|c| c.to.as_str())
    }
}

/// Something the operator should know and that is not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// A swimlane that has no `agent_service_account` after this run, so its agent cannot join.
    SwimlaneCannotJoin(String),
    /// A sentinel that has no `service_account` after this run, so it cannot join.
    SentinelCannotJoin(String),
}

/// How many rows the file lists that the database already holds as the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unchanged {
    pub projects: usize,
    pub swimlanes: usize,
    pub tenant_sets: usize,
    pub sentinels: usize,
}

/// The result of comparing the file with the database.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// In the order they are written: projects, then each swimlane followed by its tenants, then sentinels.
    pub changes: Vec<Change>,
    pub unchanged: Unchanged,
    pub absent: Absent,
    pub warnings: Vec<Warning>,
}

/// Whether the file's `value` differs from the column's `current`. A service account is compared without regard to case: Google
/// does, so `Agent@x` and `agent@x` are one account, and a live row that differs only in case is no change.
fn differs(value: &str, current: Option<&str>, ignore_case: bool) -> bool {
    current.is_none_or(|current| {
        if ignore_case {
            !current.eq_ignore_ascii_case(value)
        } else {
            current != value
        }
    })
}

/// Add `field` to `fields` if the file carries it and the database does not already hold it.
fn consider(
    fields: &mut Vec<FieldChange>,
    field: Field,
    carried: Option<&str>,
    current: Option<&str>,
    ignore_case: bool,
) {
    if let Some(value) = carried.filter(|v| differs(v, current, ignore_case)) {
        fields.push(FieldChange {
            field,
            from: current.map(str::to_owned),
            to: value.to_owned(),
        });
    }
}

/// The ids the file itself creates or declares, which a reference may name even when the database does not have them yet.
struct Declared<'a> {
    projects: BTreeSet<&'a str>,
    swimlanes: BTreeSet<&'a str>,
}

/// Compare `file` with `state`. Fails with the first reference to something that is nowhere (a project or swimlane that is neither
/// in the file nor in the database), the first new row that lacks a column it needs, or a run that would change too much.
pub fn plan(file: &SeedFile, state: &State) -> Result<Plan, SeedError> {
    let mut out = Plan {
        absent: state.absent.clone(),
        ..Plan::default()
    };
    let declared = Declared {
        projects: file.projects.iter().flatten().map(|p| p.id.as_str()).collect(),
        swimlanes: file.swimlanes.iter().flatten().map(|s| s.id.as_str()).collect(),
    };
    plan_projects(file, state, &mut out)?;
    plan_swimlanes(file, state, &declared, &mut out)?;
    plan_sentinels(file, state, &declared, &mut out)?;
    if out.changes.len() > MAX_CHANGES {
        return Err(SeedError::TooManyChanges {
            changes: out.changes.len(),
        });
    }
    Ok(out)
}

fn plan_projects(file: &SeedFile, state: &State, out: &mut Plan) -> Result<(), SeedError> {
    for (item, project) in file.projects.iter().flatten().enumerate() {
        let current = state.projects.get(&project.id).map(String::as_str);
        if current.is_none() && project.display_name.is_none() {
            return Err(SeedError::MissingOnCreate {
                field: FieldPath::field("projects", item, "display_name"),
            });
        }
        let mut fields = Vec::new();
        consider(
            &mut fields,
            Field::DisplayName,
            project.display_name.as_deref(),
            current,
            false,
        );
        if fields.is_empty() {
            out.unchanged.projects += 1;
        } else {
            out.changes.push(Change {
                kind: Kind::Project,
                op: if current.is_none() { Op::Create } else { Op::Update },
                key: project.id.clone(),
                swimlane: None,
                fields,
                tenants: Vec::new(),
            });
        }
    }
    Ok(())
}

/// The columns of a swimlane that the file carries and the row does not already hold.
fn swimlane_fields(lane: &Swimlane, row: Option<&SwimlaneRow>) -> Vec<FieldChange> {
    let mut fields = Vec::new();
    let current = |get: fn(&SwimlaneRow) -> Option<&str>| row.and_then(get);
    consider(
        &mut fields,
        Field::Project,
        lane.project.as_deref(),
        current(|r| Some(&r.project)),
        false,
    );
    consider(
        &mut fields,
        Field::DisplayName,
        lane.display_name.as_deref(),
        current(|r| Some(&r.display_name)),
        false,
    );
    consider(
        &mut fields,
        Field::Cluster,
        lane.cluster.as_deref(),
        current(|r| r.cluster.as_deref()),
        false,
    );
    consider(
        &mut fields,
        Field::Tier,
        lane.tier.as_deref(),
        current(|r| r.tier.as_deref()),
        false,
    );
    consider(
        &mut fields,
        Field::NfsServer,
        lane.nfs_server.as_deref(),
        current(|r| r.nfs_server.as_deref()),
        false,
    );
    consider(
        &mut fields,
        Field::NfsExport,
        lane.nfs_export.as_deref(),
        current(|r| r.nfs_export.as_deref()),
        false,
    );
    consider(
        &mut fields,
        Field::MountRoot,
        lane.mount_root.as_deref(),
        current(|r| r.mount_root.as_deref()),
        false,
    );
    consider(
        &mut fields,
        Field::AgentServiceAccount,
        lane.agent_service_account.as_deref(),
        current(|r| r.agent_service_account.as_deref()),
        true,
    );
    fields
}

fn plan_swimlanes(
    file: &SeedFile,
    state: &State,
    declared: &Declared<'_>,
    out: &mut Plan,
) -> Result<(), SeedError> {
    for (item, lane) in file.swimlanes.iter().flatten().enumerate() {
        let at = |field: &'static str| FieldPath::field("swimlanes", item, field);
        if let Some(project) = &lane.project
            && !declared.projects.contains(project.as_str())
            && !state.projects.contains_key(project)
        {
            return Err(SeedError::UnknownProject { field: at("project") });
        }
        let row = state.swimlanes.get(lane.id.as_str());
        if row.is_none() {
            if lane.project.is_none() {
                return Err(SeedError::MissingOnCreate { field: at("project") });
            }
            if lane.display_name.is_none() {
                return Err(SeedError::MissingOnCreate {
                    field: at("display_name"),
                });
            }
        }
        let fields = swimlane_fields(lane, row);
        if fields.is_empty() {
            out.unchanged.swimlanes += 1;
        } else {
            out.changes.push(Change {
                kind: Kind::Swimlane,
                op: if row.is_none() { Op::Create } else { Op::Update },
                key: lane.id.as_str().to_owned(),
                swimlane: Some(lane.id.clone()),
                fields,
                tenants: Vec::new(),
            });
        }
        let account = lane
            .agent_service_account
            .as_deref()
            .or_else(|| row.and_then(|r| r.agent_service_account.as_deref()));
        if account.is_none() {
            out.warnings
                .push(Warning::SwimlaneCannotJoin(lane.id.as_str().to_owned()));
        }
        plan_tenants(lane, state, out);
    }
    Ok(())
}

/// The tenants of a swimlane whose tenants the file lists: the ones the database lacks are added as one change, and the ones the
/// database has and the file does not list are reported and left alone.
fn plan_tenants(lane: &Swimlane, state: &State, out: &mut Plan) {
    let Some(listed) = &lane.tenants else { return };
    let held = state.tenants.get(lane.id.as_str());
    let added: Vec<String> = listed
        .iter()
        .map(|t| t.as_str().to_owned())
        .filter(|t| held.is_none_or(|held| !held.contains(t)))
        .collect();
    let listed: BTreeSet<&str> = listed.iter().map(domain::TenantId::as_str).collect();
    for tenant in held
        .into_iter()
        .flatten()
        .filter(|t| !listed.contains(t.as_str()))
    {
        if out.absent.tenants.len() < REPORT_LIMIT {
            out.absent
                .tenants
                .push((lane.id.as_str().to_owned(), tenant.clone()));
        } else {
            out.absent.truncated = true;
        }
    }
    if added.is_empty() {
        out.unchanged.tenant_sets += 1;
    } else {
        out.changes.push(Change {
            kind: Kind::Tenants,
            op: Op::Add,
            key: lane.id.as_str().to_owned(),
            swimlane: Some(lane.id.clone()),
            fields: Vec::new(),
            tenants: added,
        });
    }
}

fn plan_sentinels(
    file: &SeedFile,
    state: &State,
    declared: &Declared<'_>,
    out: &mut Plan,
) -> Result<(), SeedError> {
    for (item, sentinel) in file.sentinels.iter().flatten().enumerate() {
        let at = |field: &'static str| FieldPath::field("sentinels", item, field);
        if let Some(swimlane) = &sentinel.swimlane
            && !declared.swimlanes.contains(swimlane.as_str())
            && !state.swimlanes.contains_key(swimlane.as_str())
        {
            return Err(SeedError::UnknownSwimlane {
                field: at("swimlane"),
            });
        }
        let row = state.sentinels.get(&sentinel.vm);
        if row.is_none() && sentinel.export_root.is_none() {
            return Err(SeedError::MissingOnCreate {
                field: at("export_root"),
            });
        }
        let mut fields = Vec::new();
        consider(
            &mut fields,
            Field::Swimlane,
            sentinel.swimlane.as_ref().map(SwimlaneId::as_str),
            row.and_then(|r| r.swimlane.as_deref()),
            false,
        );
        consider(
            &mut fields,
            Field::ExportRoot,
            sentinel.export_root.as_deref(),
            row.map(|r| r.export_root.as_str()),
            false,
        );
        consider(
            &mut fields,
            Field::ServiceAccount,
            sentinel.service_account.as_deref(),
            row.and_then(|r| r.service_account.as_deref()),
            true,
        );
        if fields.is_empty() {
            out.unchanged.sentinels += 1;
        } else {
            let filed_under = sentinel.swimlane.clone().or_else(|| {
                row.and_then(|r| r.swimlane.as_deref())
                    .and_then(|s| SwimlaneId::parse(s).ok())
            });
            out.changes.push(Change {
                kind: Kind::Sentinel,
                op: if row.is_none() { Op::Create } else { Op::Update },
                key: sentinel.vm.clone(),
                swimlane: filed_under,
                fields,
                tenants: Vec::new(),
            });
        }
        let account = sentinel
            .service_account
            .as_deref()
            .or_else(|| row.and_then(|r| r.service_account.as_deref()));
        if account.is_none() {
            out.warnings
                .push(Warning::SentinelCannotJoin(sentinel.vm.clone()));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the audit events

/// The `diff` of an event: canonical JSON (fixed field order, sorted keys), naming the kind, what was done to which row, each column
/// set with what it was and what it became, and the SHA-256 of the file that asked for it. Configuration only: no secret is ever in
/// a seed file's rows (a service account's address names an identity, it does not authenticate one).
#[derive(Serialize)]
struct Diff<'a> {
    kind: &'static str,
    op: &'static str,
    key: &'a str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    set: BTreeMap<&'static str, Set<'a>>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    added: &'a [String],
    file_sha256: &'a str,
}

#[derive(Serialize)]
struct Set<'a> {
    from: Option<&'a str>,
    to: &'a str,
}

impl Change {
    /// The audit event of this change: `SentinelEnrolled` for a sentinel, `SettingsChanged` for the rest, by the component `hub-seed`
    /// (the system is the actor; the operator is whoever ran the Job, and `file_sha256` identifies the file they ran it with).
    pub fn event(
        &self,
        actor: &AuditActor,
        at: Timestamp,
        file_sha256: &str,
    ) -> Result<AuditEvent, SeedError> {
        let diff = Diff {
            kind: self.kind.word(),
            op: self.op.word(),
            key: &self.key,
            set: self
                .fields
                .iter()
                .map(|f| {
                    (
                        f.field.name(),
                        Set {
                            from: f.from.as_deref(),
                            to: &f.to,
                        },
                    )
                })
                .collect(),
            added: &self.tenants,
            file_sha256,
        };
        let action = match self.kind {
            Kind::Sentinel => AuditAction::SentinelEnrolled,
            _ => AuditAction::SettingsChanged,
        };
        let mut event = AuditEvent::new(at, actor.clone(), action, AuditVia::System);
        event.swimlane.clone_from(&self.swimlane);
        event.diff = Some(serde_json::to_string(&diff).map_err(|_| SeedError::Audit)?);
        Ok(event)
    }
}

impl Plan {
    /// The events of every change, in the order the changes are written, for one `record_many`.
    pub fn events(&self, at: Timestamp, file_sha256: &str) -> Result<Vec<AuditEvent>, SeedError> {
        let component = ShortText::parse(COMPONENT).map_err(|_| SeedError::Audit)?;
        let actor = AuditActor::System { component };
        self.changes
            .iter()
            .map(|change| change.event(&actor, at, file_sha256))
            .collect()
    }

    /// The report for a person: what was done (or, for a dry run, would be), what was already right, what the database has that the
    /// file does not list, and what to look at. Names, ids and column names only: no value of the file (S10).
    #[must_use]
    pub fn render(&self, dry_run: bool) -> String {
        let mut out = String::new();
        let rows = self.changes.len();
        // `write!` to a `String` cannot fail.
        if dry_run {
            out.push_str("dry run: nothing was written\n");
        } else if rows == 0 {
            out.push_str("nothing to change: the database already matches the file\n");
        } else {
            let _ = writeln!(
                out,
                "applied: {} changed, one audit event each",
                count(rows, "row", "rows")
            );
        }
        for change in &self.changes {
            let _ = match change.op {
                Op::Add => writeln!(
                    out,
                    "  add tenants {} to swimlane {}",
                    change.tenants.join(", "),
                    change.key
                ),
                op => {
                    let names: Vec<&str> = change.fields.iter().map(|f| f.field.name()).collect();
                    writeln!(
                        out,
                        "  {} {} {} ({})",
                        op.word(),
                        change.kind.word(),
                        change.key,
                        names.join(", ")
                    )
                }
            };
        }
        let u = self.unchanged;
        let _ = writeln!(
            out,
            "unchanged: {}, {}, {}, {}",
            count(u.projects, "project", "projects"),
            count(u.swimlanes, "swimlane", "swimlanes"),
            count(u.tenant_sets, "tenant set", "tenant sets"),
            count(u.sentinels, "sentinel", "sentinels"),
        );
        let a = &self.absent;
        for (word, names) in [
            ("project", &a.projects),
            ("swimlane", &a.swimlanes),
            ("sentinel", &a.sentinels),
        ] {
            if !names.is_empty() {
                let _ = writeln!(out, "not in the file, left alone: {word} {}", names.join(", "));
            }
        }
        for (swimlane, tenant) in &a.tenants {
            let _ = writeln!(
                out,
                "not in the file, left alone: tenant {tenant} of swimlane {swimlane}"
            );
        }
        if a.truncated {
            let _ = writeln!(out, "(the lists above are cut at {REPORT_LIMIT} names each)");
        }
        for warning in &self.warnings {
            let _ = match warning {
                Warning::SwimlaneCannotJoin(id) => writeln!(
                    out,
                    "warning: swimlane {id} has no agent_service_account, so its agent cannot join"
                ),
                Warning::SentinelCannotJoin(vm) => writeln!(
                    out,
                    "warning: sentinel {vm} has no service_account, so it cannot join"
                ),
            };
        }
        if dry_run {
            if rows == 0 {
                out.push_str("nothing would change\n");
            } else {
                let _ = writeln!(out, "{} would change", count(rows, "row", "rows"));
            }
        }
        out
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::file::parse;

    fn file(text: &str) -> SeedFile {
        parse(text.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    const FULL: &str = r"
version: 1
projects:
  - id: dev
    display_name: Development
swimlanes:
  - id: sl-01
    project: dev
    display_name: Swimlane 1
    cluster: gke-1
    tier: dev
    nfs_server: nfs-1
    nfs_export: /export/sl-01
    mount_root: /mnt/config
    agent_service_account: agent-1@p.iam.gserviceaccount.com
    tenants: [sit1, sit5]
  - id: sl-02
    project: dev
    display_name: Swimlane 2
    agent_service_account: agent-2@p.iam.gserviceaccount.com
    tenants: [sit2]
sentinels:
  - vm: nfs-vm-01
    swimlane: sl-01
    export_root: /export
    service_account: sentinel-1@p.iam.gserviceaccount.com
";

    /// The database after `plan` has been applied to `state`: what a second run would read.
    fn after(state: &State, plan: &Plan) -> State {
        state.after(plan)
    }

    fn keys(plan: &Plan) -> Vec<String> {
        plan.changes
            .iter()
            .map(|c| format!("{} {} {}", c.op.word(), c.kind.word(), c.key))
            .collect()
    }

    fn names(change: &Change) -> Vec<&'static str> {
        change.fields.iter().map(|f| f.field.name()).collect()
    }

    #[test]
    fn an_empty_database_gets_everything_in_the_order_it_is_written() {
        let plan = plan(&file(FULL), &State::default()).unwrap();
        assert_eq!(
            keys(&plan),
            [
                "create project dev",
                "create swimlane sl-01",
                "add swimlane_tenants sl-01",
                "create swimlane sl-02",
                "add swimlane_tenants sl-02",
                "create sentinel nfs-vm-01",
            ]
        );
        let lane = &plan.changes[1];
        assert_eq!(
            names(lane),
            [
                "project",
                "display_name",
                "cluster",
                "tier",
                "nfs_server",
                "nfs_export",
                "mount_root",
                "agent_service_account"
            ]
        );
        assert!(lane.fields.iter().all(|f| f.from.is_none()));
        assert_eq!(plan.changes[2].tenants, ["sit1", "sit5"]);
        assert_eq!(
            names(&plan.changes[5]),
            ["swimlane", "export_root", "service_account"]
        );
        assert_eq!(plan.unchanged, Unchanged::default());
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let first = plan(&file(FULL), &State::default()).unwrap();
        let state = after(&State::default(), &first);
        let second = plan(&file(FULL), &state).unwrap();
        assert!(second.changes.is_empty(), "{:?}", second.changes);
        assert_eq!(
            second.unchanged,
            Unchanged {
                projects: 1,
                swimlanes: 2,
                tenant_sets: 2,
                sentinels: 1
            }
        );
    }

    #[test]
    fn only_the_fields_the_file_carries_and_that_differ_are_changed() {
        // A running swimlane: the agent's hello filled the mount facts, an admin set the rest. The file carries only the account.
        let mut state = after(&State::default(), &plan(&file(FULL), &State::default()).unwrap());
        {
            let row = state.swimlanes.get_mut("sl-02").unwrap();
            row.cluster = Some("filled-by-the-agent".to_owned());
            row.nfs_server = Some("filled-by-the-agent".to_owned());
            row.agent_service_account = None;
            row.display_name = "Renamed by an admin".to_owned();
        }
        let only_the_account = file(
            "version: 1\nswimlanes:\n  - id: sl-02\n    agent_service_account: agent-2@p.iam.gserviceaccount.com\n",
        );
        let plan = plan(&only_the_account, &state).unwrap();
        assert_eq!(keys(&plan), ["update swimlane sl-02"]);
        assert_eq!(names(&plan.changes[0]), ["agent_service_account"]);
        assert_eq!(plan.changes[0].fields[0].from, None);
        // What is applied leaves the other columns as they were.
        let next = after(&state, &plan);
        let row = &next.swimlanes["sl-02"];
        assert_eq!(row.cluster.as_deref(), Some("filled-by-the-agent"));
        assert_eq!(row.display_name, "Renamed by an admin");
        assert_eq!(
            row.agent_service_account.as_deref(),
            Some("agent-2@p.iam.gserviceaccount.com")
        );
    }

    #[test]
    fn a_carried_field_that_differs_is_an_update_with_what_it_was() {
        let state = after(&State::default(), &plan(&file(FULL), &State::default()).unwrap());
        let changed = FULL.replace("display_name: Swimlane 1", "display_name: Swimlane One");
        let plan = plan(&file(&changed), &state).unwrap();
        assert_eq!(keys(&plan), ["update swimlane sl-01"]);
        let change = &plan.changes[0].fields[0];
        assert_eq!(change.field, Field::DisplayName);
        assert_eq!(change.from.as_deref(), Some("Swimlane 1"));
        assert_eq!(change.to, "Swimlane One");
        assert_eq!(
            plan.changes[0].swimlane.as_ref().map(SwimlaneId::as_str),
            Some("sl-01")
        );
    }

    #[test]
    fn a_service_account_that_differs_only_in_case_is_the_same_account() {
        let mut state = after(&State::default(), &plan(&file(FULL), &State::default()).unwrap());
        state.swimlanes.get_mut("sl-01").unwrap().agent_service_account =
            Some("Agent-1@P.iam.gserviceaccount.com".to_owned());
        state.sentinels.get_mut("nfs-vm-01").unwrap().service_account =
            Some("SENTINEL-1@p.iam.gserviceaccount.com".to_owned());
        assert!(plan(&file(FULL), &state).unwrap().changes.is_empty());
        // A different account is an update.
        state.swimlanes.get_mut("sl-01").unwrap().agent_service_account =
            Some("someone-else@p.iam.gserviceaccount.com".to_owned());
        let plan = plan(&file(FULL), &state).unwrap();
        assert_eq!(keys(&plan), ["update swimlane sl-01"]);
    }

    #[test]
    fn a_project_is_renamed_only_when_the_file_names_it() {
        let state = State {
            projects: BTreeMap::from([("dev".to_owned(), "Old name".to_owned())]),
            ..State::default()
        };
        let plan1 = plan(&file(FULL), &state).unwrap();
        assert_eq!(plan1.changes[0].op, Op::Update);
        assert_eq!(plan1.changes[0].fields[0].from.as_deref(), Some("Old name"));
        // Listed with only its id, a project is a declaration and nothing else.
        let plan2 = plan(&file("version: 1\nprojects:\n  - id: dev\n"), &state).unwrap();
        assert!(plan2.changes.is_empty());
        assert_eq!(plan2.unchanged.projects, 1);
    }

    #[test]
    fn what_the_database_has_and_the_file_does_not_list_is_reported_and_never_removed() {
        let mut state = after(&State::default(), &plan(&file(FULL), &State::default()).unwrap());
        state.tenants.get_mut("sl-01").unwrap().insert("sit9".to_owned());
        state.absent = Absent {
            projects: vec!["old-project".to_owned()],
            swimlanes: vec!["sl-99".to_owned()],
            sentinels: vec!["old-vm".to_owned()],
            ..Absent::default()
        };
        let plan = plan(&file(FULL), &state).unwrap();
        assert!(plan.changes.is_empty(), "{:?}", plan.changes);
        assert_eq!(plan.absent.projects, ["old-project"]);
        assert_eq!(plan.absent.swimlanes, ["sl-99"]);
        assert_eq!(plan.absent.sentinels, ["old-vm"]);
        assert_eq!(plan.absent.tenants, [("sl-01".to_owned(), "sit9".to_owned())]);
        // There is no delete anywhere in what a plan can hold.
        for change in &plan.changes {
            assert!(matches!(change.op, Op::Create | Op::Update | Op::Add));
        }
    }

    #[test]
    fn tenants_are_added_and_the_ones_the_file_does_not_carry_are_left_alone() {
        let mut state = after(&State::default(), &plan(&file(FULL), &State::default()).unwrap());
        state.tenants.get_mut("sl-01").unwrap().remove("sit5");
        state.tenants.get_mut("sl-01").unwrap().insert("sit9".to_owned());
        let plan1 = plan(&file(FULL), &state).unwrap();
        assert_eq!(keys(&plan1), ["add swimlane_tenants sl-01"]);
        assert_eq!(plan1.changes[0].tenants, ["sit5"]);
        assert_eq!(plan1.absent.tenants, [("sl-01".to_owned(), "sit9".to_owned())]);
        // A swimlane listed without `tenants` has its tenants untouched and unreported.
        let no_tenants = file("version: 1\nswimlanes:\n  - id: sl-01\n");
        let plan2 = plan(&no_tenants, &state).unwrap();
        assert!(plan2.changes.is_empty());
        assert!(plan2.absent.tenants.is_empty());
        assert_eq!(plan2.unchanged.tenant_sets, 0);
    }

    #[test]
    fn a_reference_to_something_that_is_nowhere_is_refused_at_its_place() {
        let text = "version: 1\nswimlanes:\n  - id: sl-01\n    project: nope\n    display_name: One\n";
        assert_eq!(
            plan(&file(text), &State::default()).unwrap_err(),
            SeedError::UnknownProject {
                field: FieldPath::field("swimlanes", 0, "project")
            }
        );
        // A project the file creates, or the database has, is known.
        let with_project = format!("{text}projects:\n  - id: nope\n    display_name: Nope\n");
        assert!(plan(&file(&with_project), &State::default()).is_ok());
        let state = State {
            projects: BTreeMap::from([("nope".to_owned(), "Nope".to_owned())]),
            ..State::default()
        };
        assert!(plan(&file(text), &state).is_ok());

        let sentinel = "version: 1\nsentinels:\n  - vm: v\n    swimlane: sl-77\n    export_root: /e\n";
        assert_eq!(
            plan(&file(sentinel), &State::default()).unwrap_err(),
            SeedError::UnknownSwimlane {
                field: FieldPath::field("sentinels", 0, "swimlane")
            }
        );
        let state = State {
            swimlanes: BTreeMap::from([("sl-77".to_owned(), SwimlaneRow::default())]),
            ..State::default()
        };
        assert!(plan(&file(sentinel), &state).is_ok());
    }

    #[test]
    fn a_new_row_needs_what_its_columns_require() {
        let missing = |text: &str| plan(&file(text), &State::default()).unwrap_err();
        assert_eq!(
            missing("version: 1\nprojects:\n  - id: p\n"),
            SeedError::MissingOnCreate {
                field: FieldPath::field("projects", 0, "display_name")
            }
        );
        assert_eq!(
            missing(
                "version: 1\nprojects:\n  - id: p\n    display_name: P\nswimlanes:\n  - id: sl-01\n    display_name: One\n"
            ),
            SeedError::MissingOnCreate {
                field: FieldPath::field("swimlanes", 0, "project")
            }
        );
        assert_eq!(
            missing(
                "version: 1\nprojects:\n  - id: p\n    display_name: P\nswimlanes:\n  - id: sl-01\n    project: p\n"
            ),
            SeedError::MissingOnCreate {
                field: FieldPath::field("swimlanes", 0, "display_name")
            }
        );
        assert_eq!(
            missing("version: 1\nsentinels:\n  - vm: v\n"),
            SeedError::MissingOnCreate {
                field: FieldPath::field("sentinels", 0, "export_root")
            }
        );
    }

    #[test]
    fn a_swimlane_or_a_sentinel_that_cannot_join_is_a_warning_and_not_an_error() {
        let text = "version: 1\nprojects:\n  - id: p\n    display_name: P\nswimlanes:\n  - id: sl-01\n    project: p\n    display_name: One\n  - id: sl-02\n    project: p\n    display_name: Two\n    agent_service_account: a@p.iam.gserviceaccount.com\nsentinels:\n  - vm: v\n    export_root: /e\n";
        let plan = plan(&file(text), &State::default()).unwrap();
        assert_eq!(
            plan.warnings,
            [
                Warning::SwimlaneCannotJoin("sl-01".to_owned()),
                Warning::SentinelCannotJoin("v".to_owned())
            ]
        );
        // A row that already has an account, and a file that does not mention it, is not a warning.
        let mut state = after(&State::default(), &plan);
        state.swimlanes.get_mut("sl-01").unwrap().agent_service_account =
            Some("x@p.iam.gserviceaccount.com".to_owned());
        let again = plan_of(text, &state);
        assert_eq!(again.warnings, [Warning::SentinelCannotJoin("v".to_owned())]);
    }

    fn plan_of(text: &str, state: &State) -> Plan {
        plan(&file(text), state).unwrap()
    }

    #[test]
    fn a_run_changes_at_most_two_hundred_rows() {
        let many = |n: usize| {
            let mut text = String::from("version: 1\nprojects:\n");
            for i in 0..n {
                let _ = write!(text, "  - id: p{i}\n    display_name: P{i}\n");
            }
            text
        };
        // 200 projects (the most a section may list) is exactly the bound.
        assert_eq!(plan_of(&many(200), &State::default()).changes.len(), 200);
        // One more row from another section is 201.
        let over = format!(
            "{}swimlanes:\n  - id: sl-01\n    project: p0\n    display_name: One\n",
            many(200)
        );
        assert_eq!(
            plan(&file(&over), &State::default()).unwrap_err(),
            SeedError::TooManyChanges { changes: 201 }
        );
        // Rows that need no change do not count: the same file against a database that has the projects is one change.
        let state = after(&State::default(), &plan_of(&many(200), &State::default()));
        assert_eq!(plan(&file(&over), &state).unwrap().changes.len(), 1);
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn events(plan: &Plan) -> Vec<AuditEvent> {
        plan.events(Timestamp::from_unix_millis(1_700_000_000_000), SHA)
            .unwrap()
    }

    #[test]
    fn every_change_is_exactly_one_event_by_the_seed_component() {
        let plan = plan(&file(FULL), &State::default()).unwrap();
        let events = events(&plan);
        assert_eq!(events.len(), plan.changes.len());
        for (event, change) in events.iter().zip(&plan.changes) {
            assert_eq!(
                event.actor,
                AuditActor::System {
                    component: ShortText::parse("hub-seed").unwrap()
                }
            );
            assert_eq!(event.via, AuditVia::System);
            assert_eq!(event.at, Timestamp::from_unix_millis(1_700_000_000_000));
            let want = if change.kind == Kind::Sentinel {
                AuditAction::SentinelEnrolled
            } else {
                AuditAction::SettingsChanged
            };
            assert_eq!(event.action, want);
            assert_eq!(event.swimlane, change.swimlane);
            assert!(event.path.is_none() && event.hash_before.is_none() && event.request_id.is_none());
        }
        // The project has no swimlane; the swimlane, its tenant set and the sentinel (which names one) do.
        let swimlanes: Vec<Option<&str>> = events
            .iter()
            .map(|e| e.swimlane.as_ref().map(SwimlaneId::as_str))
            .collect();
        assert_eq!(
            swimlanes,
            [
                None,
                Some("sl-01"),
                Some("sl-01"),
                Some("sl-02"),
                Some("sl-02"),
                Some("sl-01")
            ]
        );
    }

    #[test]
    fn the_diff_names_the_kind_the_change_and_the_file_in_canonical_json() {
        let plan = plan(&file(FULL), &State::default()).unwrap();
        let events = events(&plan);
        let diff: serde_json::Value = serde_json::from_str(events[1].diff.as_deref().unwrap()).unwrap();
        assert_eq!(diff["kind"], "swimlane");
        assert_eq!(diff["op"], "create");
        assert_eq!(diff["key"], "sl-01");
        assert_eq!(diff["file_sha256"], SHA);
        assert_eq!(
            diff["set"]["display_name"],
            serde_json::json!({"from": null, "to": "Swimlane 1"})
        );
        assert_eq!(
            diff["set"]["agent_service_account"]["to"],
            "agent-1@p.iam.gserviceaccount.com"
        );
        let tenants: serde_json::Value = serde_json::from_str(events[2].diff.as_deref().unwrap()).unwrap();
        assert_eq!(tenants["kind"], "swimlane_tenants");
        assert_eq!(tenants["op"], "add");
        assert_eq!(tenants["added"], serde_json::json!(["sit1", "sit5"]));
        assert!(tenants.get("set").is_none());
        // The same change makes the same bytes: the diff is canonical, so a chain verified later sees what was written.
        assert_eq!(self::events(&plan)[1].diff, events[1].diff);
        // An update says what the column was.
        let mut state = after(&State::default(), &plan);
        state.projects.insert("dev".to_owned(), "Old".to_owned());
        let update = super::plan(&file(FULL), &state).unwrap();
        let diff: serde_json::Value =
            serde_json::from_str(self::events(&update)[0].diff.as_deref().unwrap()).unwrap();
        assert_eq!(diff["op"], "update");
        assert_eq!(
            diff["set"]["display_name"],
            serde_json::json!({"from": "Old", "to": "Development"})
        );
    }

    #[test]
    fn a_full_run_fits_one_audit_batch() {
        // 200 changes, each as large as a change can be, are what `record_many` takes: the limit on a run is the bound.
        let long = "x".repeat(256);
        let mut text = String::from("version: 1\nswimlanes:\n");
        let state = State {
            projects: BTreeMap::from([("p".to_owned(), "P".to_owned())]),
            ..State::default()
        };
        for i in 0..200 {
            let _ = write!(
                text,
                "  - id: sl-{i}\n    project: p\n    display_name: \"{long}\"\n    cluster: \"{long}\"\n    tier: \"{tier}\"\n    nfs_server: nfs\n    nfs_export: /e\n    mount_root: /m\n    agent_service_account: a{i}@p.iam.gserviceaccount.com\n",
                tier = "t".repeat(64)
            );
        }
        let plan = plan(&file(&text), &state).unwrap();
        assert_eq!(plan.changes.len(), 200);
        let bytes: usize = events(&plan)
            .iter()
            .map(|e| ports::canonical_size(e).unwrap())
            .sum();
        assert!(bytes <= ports::MAX_AUDIT_BATCH_BYTES, "{bytes} bytes");
    }

    #[test]
    fn the_report_names_ids_and_columns_and_never_a_value() {
        let secretish = FULL
            .replace("Swimlane 1", "Display-Value-Planted")
            .replace("gke-1", "Cluster-Value-Planted");
        let plan = plan(&file(&secretish), &State::default()).unwrap();
        let report = plan.render(true);
        for value in [
            "Display-Value-Planted",
            "Cluster-Value-Planted",
            "agent-1@",
            "/export/sl-01",
            "nfs-1",
        ] {
            assert!(!report.contains(value), "{value} in\n{report}");
        }
    }

    #[test]
    fn the_dry_run_report_is_this() {
        let mut state = after(&State::default(), &plan(&file(FULL), &State::default()).unwrap());
        state.projects.insert("dev".to_owned(), "Old name".to_owned());
        state.swimlanes.remove("sl-02");
        state.tenants.remove("sl-02");
        state.tenants.get_mut("sl-01").unwrap().remove("sit5");
        state.tenants.get_mut("sl-01").unwrap().insert("sit9".to_owned());
        state.sentinels.get_mut("nfs-vm-01").unwrap().service_account = None;
        state.absent = Absent {
            swimlanes: vec!["sl-99".to_owned()],
            ..Absent::default()
        };
        let text = FULL.replace(
            "    agent_service_account: agent-2@p.iam.gserviceaccount.com\n",
            "",
        );
        let plan = plan(&file(&text), &state).unwrap();
        insta::assert_snapshot!(plan.render(true), @"
        dry run: nothing was written
          update project dev (display_name)
          add tenants sit5 to swimlane sl-01
          create swimlane sl-02 (project, display_name)
          add tenants sit2 to swimlane sl-02
          update sentinel nfs-vm-01 (service_account)
        unchanged: 0 projects, 1 swimlane, 0 tenant sets, 0 sentinels
        not in the file, left alone: swimlane sl-99
        not in the file, left alone: tenant sit9 of swimlane sl-01
        warning: swimlane sl-02 has no agent_service_account, so its agent cannot join
        5 rows would change
        ");
    }

    #[test]
    fn the_applied_report_and_the_empty_reports() {
        let first = plan(&file(FULL), &State::default()).unwrap();
        let applied = first.render(false);
        assert!(
            applied.starts_with("applied: 6 rows changed, one audit event each\n"),
            "{applied}"
        );
        assert!(!applied.contains("would change") && !applied.contains("dry run"));
        let state = after(&State::default(), &first);
        let second = plan(&file(FULL), &state).unwrap();
        assert!(
            second
                .render(false)
                .starts_with("nothing to change: the database already matches the file\n")
        );
        let dry = second.render(true);
        assert!(
            dry.starts_with("dry run: nothing was written\n") && dry.ends_with("nothing would change\n"),
            "{dry}"
        );
    }
}
