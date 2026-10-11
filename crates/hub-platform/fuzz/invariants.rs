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
//! `hub_forward_request` (T4), `hub_webhook_payload` (T7) and `hub_sentinel_message` (T11) are added with their tasks.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use hub_platform::ca::csr::{MAX_CSR_BYTES, parse_csr};
use prost::Message;
use proto::convert::FromAgent;
use proto::limits::{MAX_ENTRIES, MAX_FILE_BYTES, MAX_MESSAGE_BYTES, MAX_SCAN_DELTA_BYTES};
use proto::pb;
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::prelude::FromDer;

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory) and its check.
pub const TARGETS: [(&str, Check); 2] = [
    ("hub_agent_message", hub_agent_message),
    ("hub_join_csr", hub_join_csr),
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
