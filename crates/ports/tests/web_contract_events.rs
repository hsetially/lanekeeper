//! contract/08-web, `CG1a`: the payload of every streamed `DomainEvent`, the SSE framing and the byte-offset units are in
//! `api/openapi.yaml`, so that the web client types every frame and reads every range the way the server wrote it.
//!
//! The document is held to the domain: each sample below is a real `DomainEvent`, serialised by the real type, and the
//! schema of its variant must list exactly the keys that come out, all of them required (a field is never absent, a
//! nullable one is `null`), with no extra key allowed. The framing facts are the ones `hub-platform`'s `events/sse.rs` and
//! `events/wire.rs` implement; a reword of the description that drops one of them is a deliberate act, so it fails here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use domain::{
    AgentStatus, CommitId, ContentHash, DomainEvent, GitRef, JobRef, NfsPath, ObservationSource, PrJobId,
    PrJobState, PrLinkId, PrState, ProposalId, ProposalStatus, RepoKind, ServiceRef, SwimlaneId,
};
use ports::conformance::sample::user_id;
use serde_json::Value;

const SPEC: &str = include_str!("../../../api/openapi.yaml");
const SCHEMA_REF: &str = "#/components/schemas/";

/// The most entries an event's `paths` or `services` list may declare. An event is at most 4,096 bytes, so the real
/// bound is far lower; the number only has to exist and be finite (S11).
const MAX_LIST: u64 = 1000;

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..16 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return cur;
        };
        let pointer = r.strip_prefix('#').unwrap_or_else(|| panic!("external $ref {r}"));
        cur = doc
            .pointer(pointer)
            .unwrap_or_else(|| panic!("dangling $ref {r}"));
    }
    panic!("$ref chain too deep");
}

fn ref_name(v: &Value) -> &str {
    let r = v["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a $ref, got {v}"));
    r.strip_prefix(SCHEMA_REF)
        .unwrap_or_else(|| panic!("not a schema $ref: {r}"))
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema.get("required").map(strings).unwrap_or_default()
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
        Some("null") => "null".to_owned(),
        Some("integer") => format!("integer:{}", s["format"].as_str().unwrap_or("any")),
        Some("array") => format!("array<{}>", describe(&s["items"])),
        Some(other) => other.to_owned(),
        None => panic!("a property with neither $ref, oneOf nor type: {s}"),
    }
}

/// The schema of the variant whose `type` is `ty`, and its name.
fn variant<'a>(d: &'a Value, ty: &str) -> Option<(&'a str, &'a Value)> {
    d["components"]["schemas"]["DomainEvent"]["oneOf"]
        .as_array()
        .expect("DomainEvent is a oneOf")
        .iter()
        .find_map(|arm| {
            let schema = resolve(d, arm);
            (schema["properties"]["type"]["const"] == ty).then(|| (ref_name(arm), schema))
        })
}

/// Every event a browser can receive: all kinds except the internal `UserSessionsRevoked`. `GitHeadMoved` appears twice, once
/// with a `from` and once without, to show that a nullable field is present either way.
fn public_events() -> Vec<DomainEvent> {
    let sl = || SwimlaneId::parse("sit1").unwrap();
    let path = || NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap();
    let hash = || ContentHash::from_bytes([0xab; 32]);
    let commit = |c: &str| CommitId::parse(&c.repeat(40)).unwrap();
    let job = || JobRef::new("sync-job-1", "uid-1").unwrap();
    let svc = || ServiceRef::new("csp", "tx-infinity-api").unwrap();
    vec![
        DomainEvent::FileObserved {
            swimlane: sl(),
            path: path(),
            hash: hash(),
            source: ObservationSource::Scan,
        },
        DomainEvent::DriftChanged {
            swimlane: sl(),
            paths: vec![path()],
        },
        DomainEvent::FindingsChanged { swimlane: sl() },
        DomainEvent::PendingRestartChanged {
            swimlane: sl(),
            services: vec![svc()],
        },
        DomainEvent::PickupChanged {
            swimlane: sl(),
            services: vec![svc()],
        },
        DomainEvent::ProposalChanged {
            id: ProposalId::from(7),
            status: ProposalStatus::Pending,
        },
        DomainEvent::AgentStatusChanged {
            swimlane: sl(),
            status: AgentStatus::Connected,
        },
        DomainEvent::GitHeadMoved {
            repo: RepoKind::Tenant,
            branch: GitRef::parse("sit1").unwrap(),
            from: Some(commit("1")),
            to: commit("2"),
        },
        DomainEvent::GitHeadMoved {
            repo: RepoKind::Base,
            branch: GitRef::parse("main").unwrap(),
            from: None,
            to: commit("3"),
        },
        DomainEvent::AccessRequestCreated { user: user_id(1) },
        DomainEvent::DocIndexed {
            doc: domain::DocId::from(3),
        },
        DomainEvent::Resync,
        DomainEvent::AttributionUpdated {
            swimlane: sl(),
            path: path(),
            hash: hash(),
        },
        DomainEvent::PrStateChanged {
            link_id: PrLinkId::from(11),
            state: PrState::Merged,
        },
        DomainEvent::PrJobProgress {
            job_id: PrJobId::from(12),
            state: PrJobState::Committing,
        },
        DomainEvent::SyncWindowOpened {
            swimlane: sl(),
            job: job(),
        },
        DomainEvent::SyncWindowClosed {
            swimlane: sl(),
            job: job(),
        },
    ]
}

/// What each variant's fields must be, by wire name. A variant of the spec that has no sample above is held to the
/// structural rules only; the entry here is for the ones that do.
const FIELD_SHAPES: &[(&str, &[(&str, &str)])] = &[
    (
        "file_observed",
        &[
            ("swimlane", "ref:SwimlaneId"),
            ("path", "ref:NfsPath"),
            ("hash", "ref:ContentHash"),
            ("source", "ref:ObservationSource"),
        ],
    ),
    (
        "drift_changed",
        &[("swimlane", "ref:SwimlaneId"), ("paths", "array<ref:NfsPath>")],
    ),
    ("findings_changed", &[("swimlane", "ref:SwimlaneId")]),
    (
        "pending_restart_changed",
        &[
            ("swimlane", "ref:SwimlaneId"),
            ("services", "array<ref:ServiceRef>"),
        ],
    ),
    (
        "pickup_changed",
        &[
            ("swimlane", "ref:SwimlaneId"),
            ("services", "array<ref:ServiceRef>"),
        ],
    ),
    (
        "proposal_changed",
        &[("id", "integer:int64"), ("status", "ref:ProposalStatus")],
    ),
    (
        "agent_status_changed",
        &[("swimlane", "ref:SwimlaneId"), ("status", "ref:AgentStatus")],
    ),
    (
        "git_head_moved",
        &[
            ("repo", "ref:RepoKind"),
            ("branch", "ref:GitRef"),
            ("from", "ref:CommitId|null"),
            ("to", "ref:CommitId"),
        ],
    ),
    ("access_request_created", &[("user", "ref:UserId")]),
    ("doc_indexed", &[("doc", "integer:int64")]),
    ("resync", &[]),
    (
        "attribution_updated",
        &[
            ("swimlane", "ref:SwimlaneId"),
            ("path", "ref:NfsPath"),
            ("hash", "ref:ContentHash"),
        ],
    ),
    (
        "pr_state_changed",
        &[("link_id", "integer:int64"), ("state", "ref:PrState")],
    ),
    (
        "pr_job_progress",
        &[("job_id", "integer:int64"), ("state", "ref:PrJobState")],
    ),
    (
        "sync_window_opened",
        &[("swimlane", "ref:SwimlaneId"), ("job", "ref:JobRef")],
    ),
    (
        "sync_window_closed",
        &[("swimlane", "ref:SwimlaneId"), ("job", "ref:JobRef")],
    ),
];

/// Kinds that the document may have before this file has a sample for them. `swimlane_changed` is `CG1b`, a later commit of
/// the same PR, which may add a sample here but does not have to.
const UNSAMPLED_OK: &[&str] = &["swimlane_changed"];

/// `DomainEvent` is a `oneOf` with one arm per streamable kind, told apart by `type`, and the arms are exactly the
/// `EventKind` values: the internal `user_sessions_revoked` is not an arm, because it never reaches a browser.
#[test]
fn domain_event_is_a_oneof_over_exactly_the_streamable_kinds() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let event = &schemas["DomainEvent"];
    let arms = event["oneOf"]
        .as_array()
        .expect("DomainEvent must be a oneOf of one schema per kind");

    let mut by_type: BTreeMap<String, String> = BTreeMap::new();
    for arm in arms {
        let name = ref_name(arm).to_owned();
        let ty = schemas[name.as_str()]["properties"]["type"]["const"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} has no `type` const"))
            .to_owned();
        assert!(
            by_type.insert(ty.clone(), name).is_none(),
            "two arms of DomainEvent have the type `{ty}`"
        );
    }

    assert_eq!(event["discriminator"]["propertyName"], "type");
    let mapping = event["discriminator"]["mapping"]
        .as_object()
        .expect("the discriminator has a mapping");
    let mapped: BTreeMap<String, String> = mapping
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect();
    let expected: BTreeMap<String, String> = by_type
        .iter()
        .map(|(ty, name)| (ty.clone(), format!("{SCHEMA_REF}{name}")))
        .collect();
    assert_eq!(
        mapped, expected,
        "the discriminator maps each type to its own arm"
    );

    assert_eq!(
        by_type.keys().cloned().collect::<BTreeSet<_>>(),
        strings(&schemas["EventKind"]["enum"]),
        "the arms of DomainEvent are the values of EventKind, no more and no fewer"
    );
    assert!(
        !by_type.contains_key("user_sessions_revoked"),
        "an internal event is never streamed, so it has no schema"
    );
}

/// For every sample event, the schema of its variant is the domain's: the same keys, all required, nothing else allowed,
/// and each field of the declared kind. A variant with no sample is still held to "every property required, nothing else
/// allowed".
#[test]
fn domain_event_schema_matches_the_domain() {
    let d = doc();
    let events = public_events();
    let mut seen = BTreeSet::new();

    for event in &events {
        let json = serde_json::to_value(event).unwrap();
        let ty = json["type"].as_str().unwrap();
        let (name, schema) =
            variant(&d, ty).unwrap_or_else(|| panic!("DomainEvent has no arm for the kind `{ty}`"));
        seen.insert(ty.to_owned());

        assert_eq!(
            schema["additionalProperties"], false,
            "{name}: an event with a key the document does not list is a bug of the server"
        );
        let keys = wire_keys(&json);
        assert_eq!(
            declared_of(schema),
            keys,
            "{name}: the properties are the keys the domain writes"
        );
        assert_eq!(
            required_of(schema),
            keys,
            "{name}: every key is required (a nullable field is null, never absent)"
        );

        let shapes: BTreeMap<&str, &str> = FIELD_SHAPES
            .iter()
            .find(|(t, _)| *t == ty)
            .unwrap_or_else(|| panic!("FIELD_SHAPES has no row for `{ty}`"))
            .1
            .iter()
            .copied()
            .collect();
        let mut fields = keys.clone();
        fields.remove("type");
        assert_eq!(
            shapes.keys().map(|k| (*k).to_owned()).collect::<BTreeSet<_>>(),
            fields,
            "{ty}: the table lists the fields the domain writes"
        );
        for (field, want) in &shapes {
            let prop = &schema["properties"][*field];
            assert_eq!(describe(prop), *want, "{name}.{field}");
            if want.starts_with("array<") {
                let max = prop["maxItems"].as_u64();
                assert!(
                    max.is_some_and(|m| (1..=MAX_LIST).contains(&m)),
                    "{name}.{field} needs a `maxItems` of at most {MAX_LIST} (S11), got {max:?}"
                );
            }
        }
        assert_eq!(
            schema["properties"]["type"]["const"], ty,
            "{name}: the discriminator"
        );
    }

    // Every arm of the document is either sampled above or named in `UNSAMPLED_OK`, and holds the structural rules.
    let arms = d["components"]["schemas"]["DomainEvent"]["oneOf"]
        .as_array()
        .unwrap();
    for arm in arms {
        let schema = resolve(&d, arm);
        let name = ref_name(arm);
        let ty = schema["properties"]["type"]["const"].as_str().unwrap();
        assert_eq!(schema["additionalProperties"], false, "{name}");
        assert_eq!(
            required_of(schema),
            declared_of(schema),
            "{name}: every property is required"
        );
        assert!(
            seen.contains(ty) || UNSAMPLED_OK.contains(&ty),
            "{name}: `{ty}` has no sample in public_events() and is not in UNSAMPLED_OK"
        );
    }
    // A floor, not a count: the 16 kinds of the base of this PR all have a sample, and a later kind may add one.
    assert!(
        seen.len() >= 16,
        "the 16 kinds a browser could receive at the base of this PR must all be sampled, got {}",
        seen.len()
    );
}

/// `JobRef` is the domain's `JobRef`, and the inline job of `ActorJob` is the same shape.
#[test]
fn job_ref_is_the_domain_job_ref() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let job = JobRef::new("sync-job-1", "uid-1").unwrap();
    let json = serde_json::to_value(&job).unwrap();
    let schema = &schemas["JobRef"];
    assert!(schema.is_object(), "JobRef is not in the document");
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(declared_of(schema), wire_keys(&json));
    assert_eq!(required_of(schema), wire_keys(&json));

    // Bounded like the domain type (S11): the name is a Kubernetes name, the uid at most 64 of `[A-Za-z0-9-]`.
    assert_eq!(
        schema["properties"]["name"]["pattern"], schemas["ServiceRef"]["properties"]["name"]["pattern"],
        "a job name is a Kubernetes name, as a service name is"
    );
    assert_eq!(schema["properties"]["uid"]["maxLength"], 64);
    assert!(
        schema["properties"]["uid"]["pattern"].is_string(),
        "the uid needs a pattern"
    );

    let inline = &schemas["ActorJob"]["properties"]["job"];
    assert_eq!(
        declared_of(inline),
        declared_of(schema),
        "ActorJob.job and JobRef must stay the same shape"
    );
    assert_eq!(required_of(inline), required_of(schema));
}

/// Who receives what: roles are global (D35), so a Viewer sees every event of every swimlane, and the one event that only an
/// Admin receives is named, so that a Viewer's client does not wait for it. `hub-platform` pins the other direction: the events
/// that `RoleVisibility` keeps from a Viewer are exactly the ones this paragraph names.
#[test]
fn stream_events_says_who_receives_what() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let op = &d["paths"]["/api/v1/events"]["get"];
    let visibility = op["description"]
        .as_str()
        .unwrap_or("")
        .split("\n\n")
        .find(|p| p.starts_with("Visibility."))
        .expect("the description of GET /api/v1/events has a `Visibility.` paragraph")
        .to_lowercase();
    for (needle, fact) in [
        ("roles are global", "that there is no per-swimlane scope"),
        ("`access_request_created`", "the one event for Admins only"),
        ("admin", "who receives it"),
        ("viewer", "who does not"),
        ("replay", "that a replay follows the same rule"),
        ("`kinds`", "that asking for the kind does not change it"),
        ("`resync`", "that a resync always passes"),
        (
            "`user_sessions_revoked`",
            "that an internal event is never streamed",
        ),
    ] {
        assert!(
            visibility.contains(needle),
            "the Visibility paragraph of GET /api/v1/events must state {fact} (`{needle}`)"
        );
    }
    let restricted = schemas["DomainEventAccessRequestCreated"]["description"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        restricted.contains("admin") && restricted.contains("only"),
        "DomainEventAccessRequestCreated must say that only an Admin receives it"
    );
}

/// What the web client needs to know to consume the stream (T3): the frames, the resume rules, the limits.
#[test]
fn stream_events_describes_the_framing_and_the_units() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let op = &d["paths"]["/api/v1/events"]["get"];
    let ok = &op["responses"]["200"];

    // The body is typed: each `data:` line is a `DomainEvent`.
    assert_eq!(
        ok["content"]["text/event-stream"]["schema"]["$ref"],
        format!("{SCHEMA_REF}DomainEvent"),
        "the event stream must name the schema of what it carries"
    );

    let framing = [
        op["description"].as_str().unwrap_or(""),
        ok["description"].as_str().unwrap_or(""),
        op["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "Last-Event-ID")
            .and_then(|p| p["description"].as_str())
            .unwrap_or(""),
    ]
    .join("\n")
    .to_lowercase();
    for (needle, fact) in [
        ("`event:`", "the event name line is the kind"),
        ("`data:`", "the data line is the event as JSON"),
        ("`id:`", "the id line"),
        ("stored events", "that an id is on stored events only"),
        ("`resync`", "the resync event"),
        ("retry: 3000", "the reconnect delay the server sends"),
        (": heartbeat", "the keep-alive comment"),
        ("15 seconds", "the heartbeat period"),
        ("`last-event-id`", "the resume header"),
        ("1,000 events", "the most a replay carries"),
        ("without `last-event-id`", "that no header means no replay"),
        ("5 streams", "the streams one user may hold"),
        ("429", "what the sixth gets"),
        ("30 minutes", "the longest a stream lives"),
        ("4,096 bytes", "the cap that turns an event into a resync"),
    ] {
        assert!(
            framing.contains(needle),
            "the description of GET /api/v1/events must state {fact} (`{needle}`)"
        );
    }

    // The byte-offset units of every range a client reads.
    let ranges = [
        ("InlineRange", &schemas["InlineRange"]["description"]),
        (
            "TextHit.ranges",
            &schemas["TextHit"]["properties"]["ranges"]["description"],
        ),
        (
            "DiffLine.inline_changes",
            &schemas["DiffLine"]["properties"]["inline_changes"]["description"],
        ),
    ];
    for (what, text) in ranges {
        let text = text.as_str().unwrap_or("").to_lowercase();
        for (needle, fact) in [
            ("utf-8 byte", "that the unit is UTF-8 bytes"),
            ("0-based", "that the offsets start at 0"),
            ("`text`", "the string the offsets index"),
        ] {
            assert!(text.contains(needle), "{what} must state {fact} (`{needle}`)");
        }
    }
    let inline = schemas["InlineRange"]["description"]
        .as_str()
        .unwrap()
        .to_lowercase();
    for (needle, fact) in [
        ("half-open", "that `end` is exclusive"),
        ("character boundar", "that a range never splits a character"),
        (
            "characters",
            "that a secret match's location is counted in characters",
        ),
    ] {
        assert!(
            inline.contains(needle),
            "InlineRange must state {fact} (`{needle}`)"
        );
    }
    let hits = schemas["TextHit"]["properties"]["ranges"]["description"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(
        hits.contains("[start, end)"),
        "TextHit.ranges must say that a pair is `[start, end)`"
    );
}
