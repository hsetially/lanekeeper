//! The seed file (plan 18, T7, G24; rule 10, S5, S10, S11): what an operator writes to register projects, swimlanes, tenants and
//! sentinels, parsed and validated before anything else happens.
//!
//! **It is untrusted input at a trust boundary.** An operator wrote it, but it may be pasted from a chat, edited on a laptop, or
//! hold anything. So it is read with a budget (at most [`MAX_FILE_BYTES`], one document, no anchors, aliases or merge keys, a
//! shallow depth, at most [`MAX_PER_SECTION`] entries a section), with `deny_unknown_fields` at every level (a misspelt field is an
//! error and not a field silently ignored), every value is checked against the column it will be written to and against what the
//! rest of the hub does with it, and an error names a *place* (`swimlanes[2].agent_service_account`) and never a value.
//!
//! **What the file means.** It is declarative, and it is about the fields it carries:
//!
//! - a field the file does not carry is never touched, in a new row (it is left empty) or in an existing one (it keeps its value);
//! - a field cannot be cleared, only set (`null` is "not carried"), so a typo can be corrected and never lost;
//! - a row, a tenant or a sentinel that the file does not list is never deleted (the run reports it).
//!
//! The format, version 1 (YAML, or JSON, which is YAML's subset):
//!
//! ```yaml
//! version: 1
//! projects:                       # optional; each: id, display_name
//!   - id: lanekeeper-dev
//!     display_name: Lanekeeper (development)
//! swimlanes:                      # optional; each: id, project, display_name, and optionally the rest
//!   - id: sl-01
//!     project: lanekeeper-dev
//!     display_name: Swimlane 1
//!     cluster: gke-dev-1          # optional: the agent's first hello fills it when empty
//!     tier: dev                   # optional
//!     nfs_server: nfs-1.corp.example  # optional
//!     nfs_export: /export/sl-01   # optional, an absolute path
//!     mount_root: /mnt/config     # optional, an absolute path
//!     agent_service_account: agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com   # without it the swimlane's agent cannot join
//!     tenants: [sit1]             # optional: tenants are added, never removed
//! sentinels:                      # optional; each: vm, export_root, and optionally swimlane and service_account
//!   - vm: nfs-vm-01
//!     swimlane: sl-01
//!     export_root: /export
//!     service_account: sentinel-01@lanekeeper-dev.iam.gserviceaccount.com   # without it the sentinel cannot join
//! ```

use std::collections::BTreeSet;

use domain::{NfsPath, SwimlaneId, TenantId};
use hub_platform::ca::profile::PeerName;
use serde::{Deserialize, Serialize};

use super::error::{FieldPath, MAX_FILE_BYTES, Place, Reason, SeedError};

/// The one version of the format this hub reads.
pub const VERSION: u32 = 1;
/// The most entries of one section (projects, swimlanes, sentinels). The design scale is 40 swimlanes.
pub const MAX_PER_SECTION: usize = 200;
/// The most tenants one swimlane lists.
pub const MAX_TENANTS: usize = 64;
/// The longest free-text value, in bytes. `projects.display_name` and `swimlanes.display_name` are limited to 256 characters.
pub const MAX_TEXT: usize = 256;
/// The longest `tier`: the column's own check.
pub const MAX_TIER: usize = 64;
/// The longest project id: the column's own check.
pub const MAX_PROJECT_ID: usize = 64;
/// The longest path (`nfs_export`, `mount_root`, `export_root`).
pub const MAX_PATH: usize = 1024;
/// The longest email address (RFC 5321).
pub const MAX_EMAIL: usize = 254;

// ------------------------------------------------------------------------------------------------ what is read

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    version: u32,
    projects: Option<Vec<RawProject>>,
    swimlanes: Option<Vec<RawSwimlane>>,
    sentinels: Option<Vec<RawSentinel>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProject {
    id: String,
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSwimlane {
    id: String,
    project: Option<String>,
    display_name: Option<String>,
    cluster: Option<String>,
    tier: Option<String>,
    nfs_server: Option<String>,
    nfs_export: Option<String>,
    mount_root: Option<String>,
    agent_service_account: Option<String>,
    tenants: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSentinel {
    vm: String,
    swimlane: Option<String>,
    export_root: Option<String>,
    service_account: Option<String>,
}

// ------------------------------------------------------------------------------------------------ what is accepted

/// A validated seed file. Built only by [`parse`]: every value in it has been checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeedFile {
    pub version: u32,
    /// `None`: the file says nothing about projects (none are compared, none reported as absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<Project>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swimlanes: Option<Vec<Swimlane>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sentinels: Option<Vec<Sentinel>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Swimlane {
    pub id: SwimlaneId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs_server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs_export: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_service_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenants: Option<Vec<TenantId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sentinel {
    pub vm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swimlane: Option<SwimlaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account: Option<String>,
}

impl SeedFile {
    /// The file as canonical JSON, which [`parse`] reads back to an equal file. For tests and the fuzz target.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// The parser's budget for a file of at most [`MAX_FILE_BYTES`]. Everything a YAML parser can be made to expand or to recurse into
/// is switched off or held low: one document, no anchors, aliases or merge keys, no include, a shallow depth. A real file is two
/// to four levels deep and has a few thousand nodes.
fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_events: 120_000,
            max_nodes: 50_000,
            max_depth: 6,
            flow_nesting_limit: 6,
            max_aliases: 0,
            max_anchors: 0,
            max_merge_keys: 0,
            max_documents: 1,
            max_inclusion_depth: 0,
            max_total_scalar_bytes: MAX_FILE_BYTES,
            max_total_comment_bytes: MAX_FILE_BYTES,
            max_buffered_comment_events: 8,
        },
        // Comments are not read at all: their text is not kept, whatever it is.
        emit_comments: false,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
        merge_keys: serde_saphyr::MergeKeyPolicy::Error,
        // `no`, `yes`, `on`, `010` and `1e3` are what YAML 1.1 and 1.2 read differently. A value that could be retyped must be quoted,
        // so that the operator means what the hub reads (a tenant `010` is a tenant, and not the number 8).
        strict_booleans: true,
        no_schema: true,
        // `!!binary` would turn base64 into text; a value is the text that was written.
        ignore_binary_tag_for_string: true,
        // The parser's own messages quote the source line, which can hold anything.
        with_snippet: false,
    }
}

/// Read and validate a seed file. See the module's notes for what is refused. Pure: it touches no file and no database.
pub fn parse(bytes: &[u8]) -> Result<SeedFile, SeedError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(SeedError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SeedError::NotText)?;
    // A byte order mark is what some editors put in front of a file; it is not part of the first key.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let raw: RawFile =
        serde_saphyr::from_str_with_options(text, options()).map_err(|error| SeedError::Syntax {
            place: error
                .location()
                .filter(|at| at.line() > 0)
                .map_or_else(Place::unknown, |at| Place::at(at.line(), at.column())),
        })?;
    validate(raw)
}

fn validate(raw: RawFile) -> Result<SeedFile, SeedError> {
    if raw.version != VERSION {
        return Err(invalid(FieldPath::section("version"), Reason::Unsupported));
    }
    Ok(SeedFile {
        version: raw.version,
        projects: raw.projects.map(projects).transpose()?,
        swimlanes: raw.swimlanes.map(swimlanes).transpose()?,
        sentinels: raw.sentinels.map(sentinels).transpose()?,
    })
}

const fn invalid(field: FieldPath, reason: Reason) -> SeedError {
    SeedError::Invalid { field, reason }
}

/// A section may list at most [`MAX_PER_SECTION`] entries.
fn check_count(section: &'static str, count: usize) -> Result<(), SeedError> {
    if count > MAX_PER_SECTION {
        return Err(invalid(FieldPath::section(section), Reason::TooMany));
    }
    Ok(())
}

/// A value that is carried: checked with `check`, and an error is at `place`.
fn carried(
    value: Option<String>,
    place: FieldPath,
    check: impl FnOnce(&str) -> Result<(), Reason>,
) -> Result<Option<String>, SeedError> {
    match value {
        None => Ok(None),
        Some(value) => {
            check(&value).map_err(|reason| invalid(place, reason))?;
            Ok(Some(value))
        }
    }
}

fn projects(list: Vec<RawProject>) -> Result<Vec<Project>, SeedError> {
    check_count("projects", list.len())?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(list.len());
    for (item, project) in list.into_iter().enumerate() {
        let id_at = FieldPath::field("projects", item, "id");
        project_id(&project.id).map_err(|reason| invalid(id_at, reason))?;
        if !seen.insert(project.id.clone()) {
            return Err(SeedError::Duplicate { field: id_at });
        }
        out.push(Project {
            display_name: carried(
                project.display_name,
                FieldPath::field("projects", item, "display_name"),
                |v| text(v, MAX_TEXT),
            )?,
            id: project.id,
        });
    }
    Ok(out)
}

fn swimlanes(list: Vec<RawSwimlane>) -> Result<Vec<Swimlane>, SeedError> {
    check_count("swimlanes", list.len())?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(list.len());
    for (item, lane) in list.into_iter().enumerate() {
        let at = |field: &'static str| FieldPath::field("swimlanes", item, field);
        let id = SwimlaneId::parse(&lane.id).map_err(|_| invalid(at("id"), Reason::BadId))?;
        if !seen.insert(id.clone()) {
            return Err(SeedError::Duplicate { field: at("id") });
        }
        out.push(Swimlane {
            id,
            project: carried(lane.project, at("project"), project_id)?,
            display_name: carried(lane.display_name, at("display_name"), |v| text(v, MAX_TEXT))?,
            cluster: carried(lane.cluster, at("cluster"), |v| text(v, MAX_TEXT))?,
            tier: carried(lane.tier, at("tier"), |v| text(v, MAX_TIER))?,
            nfs_server: carried(lane.nfs_server, at("nfs_server"), host)?,
            nfs_export: carried(lane.nfs_export, at("nfs_export"), absolute_path)?,
            mount_root: carried(lane.mount_root, at("mount_root"), absolute_path)?,
            agent_service_account: carried(
                lane.agent_service_account,
                at("agent_service_account"),
                service_account,
            )?,
            tenants: lane
                .tenants
                .as_deref()
                .map(|list| tenants(item, list))
                .transpose()?,
        });
    }
    Ok(out)
}

fn tenants(item: usize, list: &[String]) -> Result<Vec<TenantId>, SeedError> {
    if list.len() > MAX_TENANTS {
        return Err(invalid(
            FieldPath::field("swimlanes", item, "tenants"),
            Reason::TooMany,
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(list.len());
    for (element, tenant) in list.iter().enumerate() {
        let at = FieldPath::element("swimlanes", item, "tenants", element);
        let tenant = TenantId::parse(tenant).map_err(|_| invalid(at, Reason::BadId))?;
        if !seen.insert(tenant.clone()) {
            return Err(SeedError::Duplicate { field: at });
        }
        out.push(tenant);
    }
    Ok(out)
}

fn sentinels(list: Vec<RawSentinel>) -> Result<Vec<Sentinel>, SeedError> {
    check_count("sentinels", list.len())?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(list.len());
    for (item, sentinel) in list.into_iter().enumerate() {
        let at = |field: &'static str| FieldPath::field("sentinels", item, field);
        // The name goes into the sentinel's certificate (`spiffe://lanekeeper/sentinel/<vm>`), so it must be a name the CA accepts.
        PeerName::parse(&sentinel.vm).map_err(|_| invalid(at("vm"), Reason::BadId))?;
        if !seen.insert(sentinel.vm.clone()) {
            return Err(SeedError::Duplicate { field: at("vm") });
        }
        out.push(Sentinel {
            swimlane: sentinel
                .swimlane
                .map(|id| SwimlaneId::parse(&id).map_err(|_| invalid(at("swimlane"), Reason::BadId)))
                .transpose()?,
            export_root: carried(sentinel.export_root, at("export_root"), absolute_path)?,
            service_account: carried(sentinel.service_account, at("service_account"), service_account)?,
            vm: sentinel.vm,
        });
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ the values

/// Characters that are not "control" in Unicode's sense and still change how text reads or what it looks like: zero-width and
/// direction marks and overrides (the "Trojan Source" family), and the byte order mark in the middle of a value.
fn is_format_character(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Free text for a name or a label: not empty, at most `max` bytes (the columns count characters, so bytes are stricter), no control
/// or format character, and no space at either end.
fn text(value: &str, max: usize) -> Result<(), Reason> {
    if value.trim().is_empty() {
        Err(Reason::Empty)
    } else if value.len() > max {
        Err(Reason::TooLong)
    } else if value.chars().any(|c| c.is_control() || is_format_character(c)) {
        Err(Reason::Control)
    } else if value != value.trim() {
        Err(Reason::Padded)
    } else {
        Ok(())
    }
}

/// A project id: `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`. The column only limits the length; this is what a name in a URL, a log line or a
/// label can safely be.
fn project_id(value: &str) -> Result<(), Reason> {
    let bytes = value.as_bytes();
    let first_ok = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
    let rest_ok = bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if first_ok && rest_ok && bytes.len() <= MAX_PROJECT_ID {
        Ok(())
    } else {
        Err(Reason::BadId)
    }
}

/// A host name or address (`nfs-1.corp.example`, `10.1.2.3`, `fe80::1`): letters, digits and `. _ : -`, starting with a letter or digit.
fn host(value: &str) -> Result<(), Reason> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return Err(Reason::Empty);
    }
    if bytes.len() > 253 {
        return Err(Reason::TooLong);
    }
    let first_ok = bytes[0].is_ascii_alphanumeric();
    let rest_ok = bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(Reason::BadHost)
    }
}

/// An absolute path in its clean form: what [`NfsPath`] accepts as a tree prefix and prints back unchanged, so that a value is the
/// same text whoever wrote it (no `..`, `.`, `//`, trailing slash, backslash or control character). `/` is the root.
fn absolute_path(value: &str) -> Result<(), Reason> {
    if value.is_empty() {
        return Err(Reason::Empty);
    }
    if value.len() > MAX_PATH {
        return Err(Reason::TooLong);
    }
    let Some(rest) = value.strip_prefix('/') else {
        return Err(Reason::BadPath);
    };
    if rest.is_empty() {
        return Ok(());
    }
    match NfsPath::parse_prefix(rest) {
        Ok(clean) if clean.as_str() == rest => Ok(()),
        _ => Err(Reason::BadPath),
    }
}

/// The email of a Google service account: lower-case ASCII, `local@domain`, the domain ending in `.gserviceaccount.com`
/// (`name@project.iam.gserviceaccount.com`, or `<number>-compute@developer.gserviceaccount.com`). It is compared with the `email`
/// of a Google ID token to decide who may join as an agent or a sentinel (S5), so an address of a person (`alice@gmail.com`),
/// whose token anyone in that domain can mint, is refused here and not discovered at the first join.
fn service_account(value: &str) -> Result<(), Reason> {
    if value.is_empty() {
        return Err(Reason::Empty);
    }
    if value.len() > MAX_EMAIL {
        return Err(Reason::TooLong);
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'@' | b'.' | b'_' | b'-'))
    {
        return Err(Reason::BadEmail);
    }
    let Some((local, domain)) = value.split_once('@') else {
        return Err(Reason::BadEmail);
    };
    let local_ok = !local.is_empty() && local.len() <= 64 && local.as_bytes()[0].is_ascii_alphanumeric();
    let labels_ok = domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && !label.contains('_')
            && !label.contains('@')
    });
    if !local_ok || !labels_ok || !domain.contains('.') {
        return Err(Reason::BadEmail);
    }
    if domain.ends_with(".gserviceaccount.com") {
        Ok(())
    } else {
        Err(Reason::NotServiceAccount)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const FULL: &str = r"
version: 1
projects:
  - id: lanekeeper-dev
    display_name: Lanekeeper (development)
swimlanes:
  - id: sl-01
    project: lanekeeper-dev
    display_name: Swimlane 1
    cluster: gke-dev-1
    tier: dev
    nfs_server: nfs-1.corp.example
    nfs_export: /export/sl-01
    mount_root: /mnt/config
    agent_service_account: agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com
    tenants: [sit1, sit5]
  - id: sl-02
    project: lanekeeper-dev
sentinels:
  - vm: nfs-vm-01
    swimlane: sl-01
    export_root: /export
    service_account: sentinel-01@lanekeeper-dev.iam.gserviceaccount.com
";

    fn ok(text: &str) -> SeedFile {
        parse(text.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    fn err(text: &str) -> SeedError {
        parse(text.as_bytes()).expect_err(text)
    }

    /// `FULL` with one line replaced, to try one bad value at a time.
    fn with(from: &str, to: &str) -> String {
        assert!(FULL.contains(from), "{from} is not in the file");
        FULL.replacen(from, to, 1)
    }

    fn invalid(field: FieldPath, reason: Reason) -> SeedError {
        SeedError::Invalid { field, reason }
    }

    #[test]
    fn a_complete_file_is_accepted_and_carries_what_it_says() {
        let file = ok(FULL);
        assert_eq!(file.version, 1);
        let projects = file.projects.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "lanekeeper-dev");
        assert_eq!(
            projects[0].display_name.as_deref(),
            Some("Lanekeeper (development)")
        );
        let swimlanes = file.swimlanes.unwrap();
        assert_eq!(swimlanes.len(), 2);
        let first = &swimlanes[0];
        assert_eq!(first.id.as_str(), "sl-01");
        assert_eq!(first.project.as_deref(), Some("lanekeeper-dev"));
        assert_eq!(first.nfs_export.as_deref(), Some("/export/sl-01"));
        assert_eq!(
            first.agent_service_account.as_deref(),
            Some("agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com")
        );
        let tenants: Vec<&str> = first
            .tenants
            .as_ref()
            .unwrap()
            .iter()
            .map(TenantId::as_str)
            .collect();
        assert_eq!(tenants, ["sit1", "sit5"]);
        // The second swimlane carries an id and a project, and nothing else: nothing else of it will be touched.
        let second = &swimlanes[1];
        assert_eq!(second.display_name, None);
        assert_eq!(second.agent_service_account, None);
        assert_eq!(second.tenants, None);
        let sentinels = file.sentinels.unwrap();
        assert_eq!(sentinels[0].vm, "nfs-vm-01");
        assert_eq!(
            sentinels[0].swimlane.as_ref().map(SwimlaneId::as_str),
            Some("sl-01")
        );
    }

    #[test]
    fn a_file_may_say_nothing_but_its_version() {
        let file = ok("version: 1\n");
        assert_eq!(
            (file.projects, file.swimlanes, file.sentinels),
            (None, None, None)
        );
        // An empty list is different from no list: the file says there are none (so the run reports what the database has).
        let file = ok("version: 1\nsentinels: []\n");
        assert_eq!(file.sentinels, Some(Vec::new()));
    }

    #[test]
    fn json_is_read_as_the_same_file() {
        let json = ok(FULL).to_json();
        assert!(json.starts_with('{'), "{json}");
        assert_eq!(ok(&json), ok(FULL));
    }

    #[test]
    fn a_valid_file_survives_print_and_parse() {
        for text in [
            FULL,
            "version: 1\n",
            "version: 1\nprojects:\n  - id: p\n",
            "version: 1\nswimlanes: []\n",
        ] {
            let file = ok(text);
            assert_eq!(ok(&file.to_json()), file, "{text}");
        }
    }

    #[test]
    fn null_is_a_field_the_file_does_not_carry() {
        let with_null = with("    tier: dev\n", "    tier: null\n");
        let without = with("    tier: dev\n", "");
        assert_eq!(ok(&with_null), ok(&without));
        assert_eq!(ok(&without).swimlanes.unwrap()[0].tier, None);
    }

    #[test]
    fn an_unknown_field_is_refused_at_every_level() {
        for text in [
            "version: 1\nextra: 1\n".to_owned(),
            "version: 1\nprojects:\n  - id: p\n    colour: red\n".to_owned(),
            with("    tier: dev\n", "    tier: dev\n    editing_unlocked: true\n"),
            with(
                "    service_account: sentinel",
                "    enrolled_at: x\n    service_account: sentinel",
            ),
            // A misspelling is an unknown field, not a field left out.
            with("agent_service_account:", "agent_service_acount:"),
        ] {
            assert!(matches!(err(&text), SeedError::Syntax { .. }), "{text}");
        }
    }

    #[test]
    fn the_version_is_required_and_is_one() {
        assert!(matches!(err("projects: []\n"), SeedError::Syntax { .. }));
        for version in ["0", "2", "99"] {
            assert_eq!(
                err(&format!("version: {version}\n")),
                invalid(FieldPath::section("version"), Reason::Unsupported)
            );
        }
        assert!(matches!(err("version: one\n"), SeedError::Syntax { .. }));
        assert!(matches!(err("version: -1\n"), SeedError::Syntax { .. }));
    }

    #[test]
    fn a_service_account_is_a_google_service_account_email_and_nothing_else() {
        let field = |section: &'static str, name: &'static str| FieldPath::field(section, 0, name);
        // Quoted, because an address that starts with `@` is not a YAML scalar at all.
        let bad_swimlane = |value: &str| {
            err(&with(
                "agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com",
                &format!("\"{value}\""),
            ))
        };
        for good in [
            "agent@lanekeeper-dev.iam.gserviceaccount.com",
            "123456789012-compute@developer.gserviceaccount.com",
            "a.b_c-d@p.iam.gserviceaccount.com",
        ] {
            let text = with("agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com", good);
            assert_eq!(
                ok(&text).swimlanes.unwrap()[0].agent_service_account.as_deref(),
                Some(good),
                "{good}"
            );
        }
        // A person's address is not a service account: an ID token minted for a person must never be able to join as an agent.
        for (bad, reason) in [
            ("alice@gmail.com", Reason::NotServiceAccount),
            ("alice@corp.example", Reason::NotServiceAccount),
            ("agent@gserviceaccount.com", Reason::NotServiceAccount),
            ("agent@evilgserviceaccount.com", Reason::NotServiceAccount),
            (
                "agent@p.iam.gserviceaccount.com.evil.example",
                Reason::NotServiceAccount,
            ),
            ("not-an-email", Reason::BadEmail),
            ("@p.iam.gserviceaccount.com", Reason::BadEmail),
            ("a@b@p.iam.gserviceaccount.com", Reason::BadEmail),
            ("Agent@p.iam.gserviceaccount.com", Reason::BadEmail),
            ("agent@P.iam.gserviceaccount.com", Reason::BadEmail),
            ("agent @p.iam.gserviceaccount.com", Reason::BadEmail),
            ("agent@p..iam.gserviceaccount.com", Reason::BadEmail),
            ("agent@-p.iam.gserviceaccount.com", Reason::BadEmail),
            ("agent@p.iam.gserviceaccount.com.", Reason::BadEmail),
            ("agent+tag@p.iam.gserviceaccount.com", Reason::BadEmail),
            ("agént@p.iam.gserviceaccount.com", Reason::BadEmail),
        ] {
            assert_eq!(
                bad_swimlane(bad),
                invalid(field("swimlanes", "agent_service_account"), reason),
                "{bad}"
            );
        }
        let long = format!("{}@p.iam.gserviceaccount.com", "a".repeat(250));
        assert_eq!(
            bad_swimlane(&long),
            invalid(field("swimlanes", "agent_service_account"), Reason::TooLong)
        );
        assert_eq!(
            err(&with(
                "sentinel-01@lanekeeper-dev.iam.gserviceaccount.com",
                "bob@gmail.com"
            )),
            invalid(field("sentinels", "service_account"), Reason::NotServiceAccount)
        );
        assert_eq!(
            err(&with(
                "sentinel-01@lanekeeper-dev.iam.gserviceaccount.com",
                "\"\""
            )),
            invalid(field("sentinels", "service_account"), Reason::Empty)
        );
    }

    #[test]
    fn ids_are_the_ids_the_rest_of_the_hub_uses() {
        let swimlane_id = |value: &str| err(&with("  - id: sl-01\n", &format!("  - id: \"{value}\"\n")));
        for bad in [
            "SL-01",
            "sl_01",
            "-sl",
            "sl 01",
            "sl.01",
            "sl/01",
            &"a".repeat(64),
        ] {
            assert_eq!(
                swimlane_id(bad),
                invalid(FieldPath::field("swimlanes", 0, "id"), Reason::BadId),
                "{bad}"
            );
        }
        assert_eq!(
            swimlane_id(""),
            invalid(FieldPath::field("swimlanes", 0, "id"), Reason::BadId)
        );
        let tenant = |value: &str| err(&with("[sit1, sit5]", &format!("[sit1, \"{value}\"]")));
        for bad in ["SIT1", "sit.1", "sit 1", "", "-sit", &"a".repeat(64)] {
            assert_eq!(
                tenant(bad),
                invalid(FieldPath::element("swimlanes", 0, "tenants", 1), Reason::BadId),
                "{bad}"
            );
        }
        let project = |value: &str| {
            err(&with(
                "  - id: lanekeeper-dev\n",
                &format!("  - id: \"{value}\"\n"),
            ))
        };
        for bad in ["", " p", "p q", "p/q", "-p", "_p", "p\\\\q", &"a".repeat(65)] {
            assert_eq!(
                project(bad),
                invalid(FieldPath::field("projects", 0, "id"), Reason::BadId),
                "{bad}"
            );
        }
        let vm = |value: &str| err(&with("  - vm: nfs-vm-01\n", &format!("  - vm: \"{value}\"\n")));
        for bad in [
            "",
            "nfs vm",
            "nfs/vm",
            "-nfs",
            "nfs:vm",
            "nfs@vm",
            &"a".repeat(254),
        ] {
            assert_eq!(
                vm(bad),
                invalid(FieldPath::field("sentinels", 0, "vm"), Reason::BadId),
                "{bad}"
            );
        }
        // And the edges that are fine.
        assert!(parse(with("  - id: sl-01\n", &format!("  - id: {}\n", "a".repeat(63))).as_bytes()).is_ok());
        assert!(
            parse(
                with(
                    "  - id: lanekeeper-dev\n",
                    &format!("  - id: {}\n", "A".repeat(64))
                )
                .as_bytes()
            )
            .is_ok()
        );
        assert!(parse(with("  - vm: nfs-vm-01\n", "  - vm: Nfs-VM_01.corp.example\n").as_bytes()).is_ok());
    }

    #[test]
    fn a_swimlane_that_names_a_sentinel_swimlane_must_be_an_id() {
        assert_eq!(
            err(&with("    swimlane: sl-01\n", "    swimlane: SL-01\n")),
            invalid(FieldPath::field("sentinels", 0, "swimlane"), Reason::BadId)
        );
    }

    #[test]
    fn free_text_is_bounded_and_has_no_control_characters() {
        let name = |value: &str| {
            err(&with(
                "display_name: Swimlane 1",
                &format!("display_name: \"{value}\""),
            ))
        };
        let place = FieldPath::field("swimlanes", 0, "display_name");
        assert_eq!(name(""), invalid(place, Reason::Empty));
        assert_eq!(name(&"x".repeat(257)), invalid(place, Reason::TooLong));
        assert_eq!(name("a\\nb"), invalid(place, Reason::Control));
        assert_eq!(name("a\\u0000b"), invalid(place, Reason::Control));
        assert_eq!(name("a\\u007fb"), invalid(place, Reason::Control));
        // Direction overrides and zero-width characters change how a name reads without being control characters ("Trojan Source").
        for sneaky in ["a\\u202eb", "a\\u200bb", "a\\u2066b", "a\\ufeffb"] {
            assert_eq!(name(sneaky), invalid(place, Reason::Control), "{sneaky}");
        }
        assert_eq!(name(" padded"), invalid(place, Reason::Padded));
        assert_eq!(name("padded "), invalid(place, Reason::Padded));
        // 256 bytes is fine, 129 two-byte characters (258 bytes) is not: the limit is in bytes, which is stricter than the column's.
        assert!(
            parse(
                with(
                    "display_name: Swimlane 1",
                    &format!("display_name: \"{}\"", "x".repeat(256))
                )
                .as_bytes()
            )
            .is_ok()
        );
        assert_eq!(name(&"\u{e9}".repeat(129)), invalid(place, Reason::TooLong));
        // The tier is the column's own 64.
        let tier = |value: &str| err(&with("tier: dev", &format!("tier: \"{value}\"")));
        assert_eq!(
            tier(&"t".repeat(65)),
            invalid(FieldPath::field("swimlanes", 0, "tier"), Reason::TooLong)
        );
        assert!(parse(with("tier: dev", &format!("tier: \"{}\"", "t".repeat(64))).as_bytes()).is_ok());
        let cluster = |value: &str| err(&with("cluster: gke-dev-1", &format!("cluster: \"{value}\"")));
        assert_eq!(
            cluster("a\\tb"),
            invalid(FieldPath::field("swimlanes", 0, "cluster"), Reason::Control)
        );
    }

    #[test]
    fn hosts_and_paths_are_what_the_hub_can_use() {
        let host = |value: &str| {
            err(&with(
                "nfs_server: nfs-1.corp.example",
                &format!("nfs_server: \"{value}\""),
            ))
        };
        let place = FieldPath::field("swimlanes", 0, "nfs_server");
        for bad in ["", "nfs 1", "nfs/1", "nfs@1", "-nfs", "nfs\\n"] {
            assert!(
                matches!(host(bad), SeedError::Invalid { field, .. } if field == place),
                "{bad}"
            );
        }
        assert!(parse(with("nfs_server: nfs-1.corp.example", "nfs_server: 10.1.2.3").as_bytes()).is_ok());
        assert!(parse(with("nfs_server: nfs-1.corp.example", "nfs_server: \"fe80::1\"").as_bytes()).is_ok());

        let path =
            |name: &'static str, from: &str, value: &str| err(&with(from, &format!("{name}: \"{value}\"")));
        for bad in [
            "export/sl-01",
            "/export/../etc",
            "/export/./sl",
            "/export//sl",
            "/export/sl/",
            "",
            "/a\\\\b",
            "/a\\u0000b",
            "~/x",
        ] {
            let got = path("nfs_export", "nfs_export: /export/sl-01", bad);
            let want = FieldPath::field("swimlanes", 0, "nfs_export");
            assert!(
                matches!(got, SeedError::Invalid { field, .. } if field == want),
                "{bad}: {got:?}"
            );
        }
        assert!(parse(with("nfs_export: /export/sl-01", "nfs_export: /").as_bytes()).is_ok());
        let root = path("export_root", "export_root: /export", "relative");
        assert_eq!(
            root,
            invalid(FieldPath::field("sentinels", 0, "export_root"), Reason::BadPath)
        );
        let mount = path("mount_root", "mount_root: /mnt/config", "../mnt");
        assert_eq!(
            mount,
            invalid(FieldPath::field("swimlanes", 0, "mount_root"), Reason::BadPath)
        );
        let long = format!("/{}", "a/".repeat(600));
        assert!(matches!(
            path("mount_root", "mount_root: /mnt/config", &long),
            SeedError::Invalid { .. }
        ));
    }

    #[test]
    fn an_id_listed_twice_is_refused_at_its_second_place() {
        assert_eq!(
            err("version: 1\nprojects:\n  - id: p\n  - id: q\n  - id: p\n"),
            SeedError::Duplicate {
                field: FieldPath::field("projects", 2, "id")
            }
        );
        assert_eq!(
            err("version: 1\nswimlanes:\n  - id: sl-01\n  - id: sl-02\n  - id: sl-01\n"),
            SeedError::Duplicate {
                field: FieldPath::field("swimlanes", 2, "id")
            }
        );
        assert_eq!(
            err("version: 1\nsentinels:\n  - vm: a\n  - vm: a\n"),
            SeedError::Duplicate {
                field: FieldPath::field("sentinels", 1, "vm")
            }
        );
        assert_eq!(
            err(&with("[sit1, sit5]", "[sit1, sit5, sit1]")),
            SeedError::Duplicate {
                field: FieldPath::element("swimlanes", 0, "tenants", 2)
            }
        );
        // The same name in two different sections is not a duplicate: a project and a swimlane are different things.
        assert!(parse(b"version: 1\nprojects:\n  - id: sl-01\nswimlanes:\n  - id: sl-01\n").is_ok());
    }

    #[test]
    fn the_limits_hold() {
        // Too many entries.
        let many: String = (0..=MAX_PER_SECTION).fold("version: 1\nprojects:\n".to_owned(), |mut s, i| {
            let _ = writeln!(s, "  - id: p{i}");
            s
        });
        assert_eq!(
            err(&many),
            invalid(FieldPath::section("projects"), Reason::TooMany)
        );
        let exactly: String = (0..MAX_PER_SECTION).fold("version: 1\nprojects:\n".to_owned(), |mut s, i| {
            let _ = writeln!(s, "  - id: p{i}");
            s
        });
        assert_eq!(ok(&exactly).projects.unwrap().len(), MAX_PER_SECTION);
        let tenants = (0..=MAX_TENANTS)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            err(&with("[sit1, sit5]", &format!("[{tenants}]"))),
            invalid(FieldPath::field("swimlanes", 0, "tenants"), Reason::TooMany)
        );
        // Too large a file: refused before it is parsed.
        let big = format!("version: 1\n# {}\n", "x".repeat(MAX_FILE_BYTES));
        assert_eq!(err(&big), SeedError::TooLarge);
        let edge = format!("version: 1\n# {}\n", "x".repeat(MAX_FILE_BYTES - 16));
        assert!(edge.len() <= MAX_FILE_BYTES);
        assert!(parse(edge.as_bytes()).is_ok());
    }

    #[test]
    fn what_a_parser_can_be_made_to_expand_is_refused() {
        // Anchors and aliases (a billion laughs), merge keys, a second document, deep nesting and a tag are all refused.
        let bomb = "version: 1\nprojects:\n  - &a {id: p}\n  - *a\n";
        assert!(matches!(err(bomb), SeedError::Syntax { .. }), "aliases");
        assert!(matches!(
            err("version: 1\nprojects:\n  - <<: {id: p}\n"),
            SeedError::Syntax { .. }
        ));
        assert!(matches!(
            err("version: 1\n---\nversion: 1\n"),
            SeedError::Syntax { .. }
        ));
        let deep = format!("version: 1\nprojects: {}{}\n", "[".repeat(100), "]".repeat(100));
        assert!(matches!(err(&deep), SeedError::Syntax { .. }));
        // A tag does not change what a value is: `!!binary` is not decoded, so the id is the text that was written.
        let tagged = ok("version: 1\nprojects:\n  - id: !!binary cA\n");
        assert_eq!(tagged.projects.unwrap()[0].id, "cA");
        // Nothing is read from anywhere else: an include is a tag, and tags are not read.
        assert!(matches!(
            err("version: 1\nprojects: !include /etc/passwd\n"),
            SeedError::Syntax { .. }
        ));
    }

    #[test]
    fn text_that_is_not_a_document_is_refused() {
        assert_eq!(parse(&[0xff, 0xfe, 0x00]).unwrap_err(), SeedError::NotText);
        for text in [
            "",
            "   \n",
            "# only a comment\n",
            "just a string",
            "- a\n- b\n",
            "{",
            "[]",
            "null",
            "42",
        ] {
            assert!(matches!(err(text), SeedError::Syntax { .. }), "{text:?}");
        }
        // A byte order mark is what some editors add, and is not part of the first key.
        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend_from_slice(b"version: 1\n");
        assert!(parse(&bom).is_ok());
    }

    #[test]
    fn a_number_or_a_word_that_yaml_would_retype_must_be_quoted() {
        // A tenant `010` read as a number, or `no` read as false, would be a different tenant than the one written: refused,
        // so the operator quotes it and means it.
        assert!(matches!(
            err(&with("[sit1, sit5]", "[sit1, 010]")),
            SeedError::Syntax { .. }
        ));
        assert!(matches!(
            err(&with("tier: dev", "tier: 1")),
            SeedError::Syntax { .. }
        ));
        assert!(matches!(
            err(&with("tier: dev", "tier: 1e3")),
            SeedError::Syntax { .. }
        ));
        // `no` is a word and stays one (YAML 1.2): the file is read by this parser only, and it never turns a word into a boolean.
        assert_eq!(
            ok(&with("tier: dev", "tier: no")).swimlanes.unwrap()[0]
                .tier
                .as_deref(),
            Some("no")
        );
        assert!(parse(with("tier: dev", "tier: \"1\"").as_bytes()).is_ok());
        assert!(parse(with("[sit1, sit5]", "[sit1, \"010\"]").as_bytes()).is_ok());
    }

    #[test]
    fn a_duplicate_key_is_an_error_and_not_the_last_one_winning() {
        assert!(matches!(
            err("version: 1\nversion: 1\n"),
            SeedError::Syntax { .. }
        ));
        assert!(matches!(
            err(&with("    tier: dev\n", "    tier: dev\n    tier: prod\n")),
            SeedError::Syntax { .. }
        ));
    }

    #[test]
    fn no_error_repeats_a_value_of_the_file() {
        // The values below are in places that are refused for each kind of reason; none may appear in the message.
        // Characters that no id, tenant, host, path or address may hold, so that each value below is refused; compared in lower case.
        let planted = "Hunter2.planted/secret";
        for text in [
            format!("version: 1\nswimlanes:\n  - id: {planted}\n"),
            format!(
                "version: 1\nswimlanes:\n  - id: sl-01\n    agent_service_account: {planted}@gmail.com\n"
            ),
            format!("version: 1\nswimlanes:\n  - id: sl-01\n    display_name: \"{planted}\\n\"\n"),
            format!("version: 1\nswimlanes:\n  - id: sl-01\n    {planted}: 1\n"),
            format!("version: 1\nswimlanes:\n  - id: sl-01\n    tier: [{planted}]\n"),
            format!("version: 1\nswimlanes:\n  - id: sl-01\n    tenants: [{planted}]\n"),
            format!("version: 1\nswimlanes:\n  - id: sl-01\n    nfs_export: {planted}\n"),
            format!("version: 1\nsentinels:\n  - vm: {planted}/x\n"),
            format!("version: {planted}\n"),
            planted.to_owned(),
            format!("version: 1\nprojects:\n  - id: {planted}\n  - id: {planted}\n"),
            // A duplicate of an id that is valid: the place is named, and the id is not.
            "version: 1\nprojects:\n  - id: hunter2\n  - id: hunter2\n".to_owned(),
            "version: 1\nswimlanes:\n  - id: sl-01\n    tenants: [hunter2, hunter2]\n".to_owned(),
            "version: 1\nsentinels:\n  - vm: hunter2\n  - vm: hunter2\n".to_owned(),
        ] {
            let error = err(&text);
            let shown = format!("{error} {error:?}").to_lowercase();
            assert!(!shown.contains("hunter2"), "{text}\n=> {shown}");
        }
    }

    proptest::proptest! {
        /// Any bytes are a file or one of the typed errors, and an error never repeats a value (the values below are made of a
        /// marker no template contains).
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..400)) {
            let _ = parse(&bytes);
        }

        #[test]
        fn arbitrary_text_in_a_value_never_panics_and_is_never_repeated(value in "[ -~\\n]{0,60}") {
            let marker = format!("zz9q{}", value.replace('\n', " "));
            for template in [
                format!("version: 1\nswimlanes:\n  - id: sl-01\n    display_name: \"{}\"\n", marker.replace('"', "'").replace('\\', "/")),
                format!("version: 1\nswimlanes:\n  - id: sl-01\n    agent_service_account: \"{}\"\n", marker.replace('"', "'").replace('\\', "/")),
            ] {
                if let Err(error) = parse(template.as_bytes()) {
                    let shown = format!("{error} {error:?}");
                    proptest::prop_assert!(!shown.contains("zz9q"), "{}", shown);
                }
            }
        }

        /// A valid file survives a print and a parse.
        #[test]
        fn a_generated_valid_file_survives_print_and_parse(
            ids in proptest::collection::btree_set("[a-z0-9][a-z0-9-]{0,10}", 0..5),
            tenants in proptest::collection::btree_set("[a-z0-9][a-z0-9_-]{0,8}", 0..4),
        ) {
            let mut text = String::from("version: 1\nswimlanes:\n");
            for id in &ids {
                let listed = tenants.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ");
                let _ = write!(text, "  - id: \"{id}\"\n    tenants: [{listed}]\n");
            }
            if ids.is_empty() {
                text = "version: 1\n".to_owned();
            }
            let file = parse(text.as_bytes()).unwrap();
            proptest::prop_assert_eq!(parse(file.to_json().as_bytes()).unwrap(), file);
        }
    }
}
