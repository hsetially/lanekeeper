//! The properties the hub's fuzz targets assert (S11, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/hub-platform/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify` runs)
//! check exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a panic,
//! which libFuzzer reports as a crash.
//!
//! Inputs that a parser rightly rejects are not violations: the checks look at what was accepted.
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `hub_agent_message` | a protobuf `AgentMessage` | decoding and validation never panic; an accepted message re-encodes and converts back to the same value, respects every size limit, and names only typed values (swimlane ids, paths, hashes) |
//! | `hub_join_csr` | a certificate signing request, DER | parsing never panics; an accepted request is at most 16 KiB, holds an uncompressed P-256 point, and is signed by its own key |
//!
//! | `hub_forward_request` | a protobuf `ForwardRequest` from a peer replica | decoding and validation never panic; an accepted request names a valid swimlane and a positive epoch, waits a bounded time, carries a command within every size limit, and survives a round trip unchanged |
//! | `hub_webhook_payload` | the JSON body of a GitHub `push` or `pull_request` delivery | parsing never panics; an accepted push names a repository, a branch or tag and commit ids that are each well formed and bounded, and a zero `after` is a deletion; an accepted pull request has a positive number that fits the column; the signature check accepts a header only when it is exactly the HMAC-SHA256 of the body (checked against a second implementation) |
//!
//! `hub_sentinel_message` (T11) is added with its task.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use hub_platform::ca::csr::{MAX_CSR_BYTES, parse_csr};
use hub_platform::gateway::wire::{ForwardCall, MAX_FORWARD_BYTES, MAX_FORWARD_TIMEOUT, pb::ForwardRequest};
use hub_platform::git::payload::{PullRequestEvent, PushEvent, PushedRef, RepoFullName};
use hub_platform::git::webhook::{DEFAULT_MAX_BODY, verify_signature};
use prost::Message;
use proto::convert::FromAgent;
use proto::limits::{MAX_ENTRIES, MAX_FILE_BYTES, MAX_MESSAGE_BYTES, MAX_NOTIFY_PATHS, MAX_SCAN_DELTA_BYTES};
use proto::pb;
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::prelude::FromDer;

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory) and its check.
pub const TARGETS: [(&str, Check); 4] = [
    ("hub_agent_message", hub_agent_message),
    ("hub_join_csr", hub_join_csr),
    ("hub_forward_request", hub_forward_request),
    ("hub_webhook_payload", hub_webhook_payload),
];

/// Every seed corpus must hold at least this many inputs, so deleting the seeds cannot go unnoticed.
pub const MIN_SEEDS: usize = 8;

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

/// Any bytes decoded as an `AgentMessage` and validated, as the hub does on a `Connect` stream.
pub fn hub_agent_message(data: &[u8]) -> Result<(), String> {
    // The transport refuses what is larger than this before decoding; the fuzzer is held to the same bound.
    if data.len() > MAX_MESSAGE_BYTES {
        return Ok(());
    }
    let Ok(message) = pb::AgentMessage::decode(data) else {
        return Ok(());
    };
    let parsed = match FromAgent::from_proto(message) {
        Ok(Some(parsed)) => parsed,
        Ok(None) | Err(_) => return Ok(()),
    };

    // An accepted message respects every limit the contract states.
    match &parsed {
        FromAgent::Delta(delta) => {
            ensure(delta.entries.len() <= MAX_ENTRIES, || {
                "too many entries accepted".into()
            })?;
            ensure(delta.removed.len() <= MAX_ENTRIES, || {
                "too many removals accepted".into()
            })?;
            let bytes = delta.payload_bytes();
            ensure(bytes <= MAX_SCAN_DELTA_BYTES, || {
                format!("{bytes} file bytes accepted in one delta")
            })?;
        }
        FromAgent::Reply(
            domain::AgentReply::File { bytes, .. } | domain::AgentReply::Served { bytes, .. },
        ) => {
            ensure(bytes.len() <= MAX_FILE_BYTES, || {
                "an oversize reply was accepted".into()
            })?;
        }
        FromAgent::CertRenewal { csr_der } => {
            ensure(!csr_der.is_empty() && csr_der.len() <= MAX_CSR_BYTES, || {
                "a renewal request outside its size limits was accepted".into()
            })?;
        }
        FromAgent::Hello(hello) => {
            ensure(hello.swimlane.as_str().len() <= 63, || {
                "a swimlane id over 63 bytes".into()
            })?;
        }
        FromAgent::Heartbeat(_) | FromAgent::Cluster(_) | FromAgent::Reply(_) => {}
    }

    // What was accepted survives a round trip unchanged: the typed value says what the wire said.
    let encoded = parsed.clone().into_proto().encode_to_vec();
    ensure(encoded.len() <= MAX_MESSAGE_BYTES, || {
        "an accepted message re-encodes past the size limit".into()
    })?;
    let again = pb::AgentMessage::decode(encoded.as_slice())
        .map_err(|e| format!("re-encoded message does not decode: {e}"))?;
    match FromAgent::from_proto(again) {
        Ok(Some(back)) => ensure(back == parsed, || {
            "an accepted message changed in a round trip".into()
        }),
        other => Err(format!(
            "an accepted message is refused after a round trip: {other:?}"
        )),
    }
}

/// Any bytes as a certificate signing request.
pub fn hub_join_csr(data: &[u8]) -> Result<(), String> {
    let Ok(csr) = parse_csr(data) else {
        return Ok(());
    };
    ensure(data.len() <= MAX_CSR_BYTES, || {
        "an oversize request was accepted".into()
    })?;
    let point = csr.public_key_point();
    ensure(point.len() == 65 && point[0] == 4, || {
        "an accepted request does not hold an uncompressed P-256 point".into()
    })?;
    // Independently of the parser's own check: the request really is signed by the key it carries.
    let (rest, request) = X509CertificationRequest::from_der(data)
        .map_err(|_| "accepted bytes do not parse again".to_owned())?;
    ensure(rest.is_empty(), || {
        "an accepted request has bytes after it".into()
    })?;
    ensure(request.verify_signature().is_ok(), || {
        "an accepted request is not signed by its own key".into()
    })?;
    ensure(
        request
            .certification_request_info
            .subject_pki
            .subject_public_key
            .data
            .as_ref()
            == point,
        || "the accepted key is not the request's key".into(),
    )
}

/// Any bytes decoded as a `ForwardRequest` and validated, as the replica service does for a peer.
pub fn hub_forward_request(data: &[u8]) -> Result<(), String> {
    // The transport refuses what is larger than this before decoding; the fuzzer is held to the same bound.
    if data.len() > MAX_FORWARD_BYTES {
        return Ok(());
    }
    let Ok(request) = ForwardRequest::decode(data) else {
        return Ok(());
    };
    let Ok(call) = ForwardCall::try_from(request) else {
        return Ok(());
    };

    ensure(
        domain::SwimlaneId::parse(call.swimlane.as_str()).is_ok() && call.swimlane.as_str().len() <= 63,
        || "an accepted request names a swimlane that is not a valid id".into(),
    )?;
    ensure(call.epoch.get() > 0, || "an accepted request has no epoch".into())?;
    ensure(
        !call.timeout.is_zero() && call.timeout <= MAX_FORWARD_TIMEOUT,
        || format!("an accepted request waits {:?}", call.timeout),
    )?;
    match &call.command {
        domain::HubCommand::WriteFile { bytes, .. } => ensure(bytes.len() <= MAX_FILE_BYTES, || {
            "an oversize write was accepted".into()
        })?,
        domain::HubCommand::NotifyConfigServer { paths, .. } => {
            ensure(paths.len() <= MAX_NOTIFY_PATHS, || {
                "too many notify paths were accepted".into()
            })?
        }
        _ => {}
    }

    // What was accepted survives a round trip unchanged.
    let encoded = call.clone().into_proto().encode_to_vec();
    ensure(encoded.len() <= MAX_FORWARD_BYTES, || {
        "an accepted request re-encodes past the size limit".into()
    })?;
    let again = ForwardRequest::decode(encoded.as_slice())
        .map_err(|e| format!("re-encoded request does not decode: {e}"))?;
    match ForwardCall::try_from(again) {
        Ok(back) => ensure(back == call, || {
            "an accepted request changed in a round trip".into()
        }),
        Err(error) => Err(format!(
            "an accepted request is refused after a round trip: {error}"
        )),
    }
}

/// SHA-256 over `data`, by a second implementation of nothing but the hash: the HMAC around it is written out below (RFC 2104),
/// so the check does not use the code it checks.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut block = [0_u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(data);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Any bytes as the body of a `push` or `pull_request` delivery (after its signature was checked), and as the inputs of the
/// signature check itself.
pub fn hub_webhook_payload(data: &[u8]) -> Result<(), String> {
    // The handler refuses a body larger than this before it parses anything.
    if data.len() > DEFAULT_MAX_BODY {
        return Ok(());
    }
    if let Ok(push) = PushEvent::parse(data) {
        ensure(
            RepoFullName::parse(push.repository.as_str()).is_ok() && push.repository.as_str().len() <= 140,
            || "an accepted push names a repository that is not an owner/name".into(),
        )?;
        let (PushedRef::Branch(name) | PushedRef::Tag(name)) = &push.target;
        let n = name.as_str();
        ensure(
            !n.is_empty()
                && n.len() <= 255
                && !n.contains("..")
                && !n.starts_with('-')
                && !n.starts_with('/')
                && !n.chars().any(char::is_control)
                && domain::GitRef::parse(n).is_ok(),
            || "an accepted push names a ref that is not well formed".into(),
        )?;
        ensure(
            !(n.len() >= 40 && n.bytes().all(|c| c.is_ascii_hexdigit())),
            || "an accepted push names a ref that is a bare commit id".into(),
        )?;
        let hex_id =
            |id: &str| (id.len() == 40 || id.len() == 64) && id.bytes().all(|c| c.is_ascii_hexdigit());
        ensure(hex_id(push.after.as_str()), || {
            "an accepted push has a commit id that is not hex".into()
        })?;
        if let Some(before) = &push.before {
            ensure(hex_id(before.as_str()), || {
                "an accepted push has a before id that is not hex".into()
            })?;
            ensure(!before.as_str().bytes().all(|c| c == b'0'), || {
                "an all-zero before id was kept as a real one".into()
            })?;
        }
        if push.after.as_str().bytes().all(|c| c == b'0') {
            ensure(push.deleted, || {
                "an all-zero after id is not marked as a deletion".into()
            })?;
        }
    }
    if let Ok(event) = PullRequestEvent::parse(data) {
        ensure((1..=i32::MAX as u32).contains(&event.number), || {
            format!("an accepted pull request has the number {}", event.number)
        })?;
        ensure(RepoFullName::parse(event.repository.as_str()).is_ok(), || {
            "an accepted pull request names a repository that is not an owner/name".into()
        })?;
    }

    // The signature check, driven by the same bytes: the first half is a secret, the second the body.
    let (secret, body) = data.split_at(data.len() / 2);
    let genuine = format!("sha256={}", hex::encode(hmac_sha256(secret, body)));
    ensure(verify_signature(secret, body, Some(&genuine)), || {
        "the genuine signature was refused".into()
    })?;
    let mut tampered = genuine.clone().into_bytes();
    let at = "sha256=".len() + data.first().map_or(0, |b| usize::from(*b)) % 64;
    tampered[at] = if tampered[at] == b'0' { b'1' } else { b'0' };
    let tampered = String::from_utf8(tampered).map_err(|e| e.to_string())?;
    ensure(!verify_signature(secret, body, Some(&tampered)), || {
        "a signature with one digit changed was accepted".into()
    })?;
    // Whatever text arrives as the header, it is accepted only if it is exactly the genuine one.
    let header = String::from_utf8_lossy(body);
    if verify_signature(secret, body, Some(&header)) {
        ensure(header.eq_ignore_ascii_case(&genuine), || {
            "a header that is not the genuine signature was accepted".into()
        })?;
    }
    ensure(!verify_signature(secret, body, None), || {
        "a missing header was accepted".into()
    })?;
    Ok(())
}
