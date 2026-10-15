//! The properties the write path's fuzz targets assert (S11, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/hub-writes/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify` runs) check
//! exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a panic, which libFuzzer
//! reports as a crash. Input that is rightly refused is not a violation: the checks look at what was accepted.
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `writes_request_json` | bytes read as the JSON body of an edit, a revert and a restart, and as the lines `path`, `expected`, `preview_id` of a delete | reading never panics; an accepted request names a valid path and typed hashes, holds no more content than the input had, and survives a round trip through its canonical JSON (or lines) unchanged |
//! | `writes_upload_multipart` | a content type, a line end, and a multipart body, read as an upload form and as a preview form | reading never panics; an accepted request has a valid path, a file within the limit, an `Expected` that agrees with its fields, and survives a round trip through a canonical form unchanged |
//! | `writes_preview_id` | text read as a preview id presented with a fixed request at a fixed time | checking never panics; an id is accepted only if it is exactly the one the service issues for that request at the second it names (so each request has one id for each second), and that second is within the ten minutes before now |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use bytes::Bytes;
use domain::{Expected, PreviewChange, SwimlaneId};
use hub_writes::fuzz_hooks;
use hub_writes::routes::wire::{
    delete_request, edit_request, preview_request, restart_request, revert_request, upload_request,
};
use serde_json::json;

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Whether the input is one the target's reader accepts, for the names of the seeds.
pub type Accepts = fn(&[u8]) -> bool;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory), its check and its reader.
pub const TARGETS: [(&str, Check, Accepts); 3] = [
    ("writes_request_json", writes_request_json, request_json_accepted),
    (
        "writes_upload_multipart",
        writes_upload_multipart,
        upload_multipart_accepted,
    ),
    ("writes_preview_id", writes_preview_id, preview_id_accepted),
];

/// Every seed corpus must hold at least this many inputs, so deleting the seeds cannot go unnoticed.
pub const MIN_SEEDS: usize = 8;

/// The most a file of a form may hold in these targets.
pub const MAX_FILE: usize = 64 * 1024;

/// The key and the time of the preview target.
pub const PREVIEW_KEY: &[u8] = b"fuzz-preview-key-0123456789abcdef";
pub const NOW_MS: i64 = 1_791_936_000_000;

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

fn swimlane() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

// ------------------------------------------------------------------------------------------------------ request json

/// The delete reader takes the input as lines: the path, the hash and, optionally, the preview id.
fn delete_lines(data: &[u8]) -> Option<(String, String, Option<String>)> {
    let text = String::from_utf8_lossy(data).into_owned();
    let mut lines = text.split('\n');
    let path = lines.next()?.to_owned();
    let expected = lines.next()?.to_owned();
    let preview = lines.next().map(str::to_owned);
    Some((path, expected, preview))
}

pub fn request_json_accepted(data: &[u8]) -> bool {
    edit_request(swimlane(), data).is_ok()
        || revert_request(swimlane(), data).is_ok()
        || restart_request(swimlane(), data).is_ok()
        || delete_lines(data)
            .is_some_and(|(p, e, v)| delete_request(swimlane(), &p, &e, v.as_deref()).is_ok())
}

pub fn writes_request_json(data: &[u8]) -> Result<(), String> {
    if let Ok(r) = edit_request(swimlane(), data) {
        ensure(r.content.len() <= data.len(), || {
            "an edit holds more content than it was sent".into()
        })?;
        ensure(
            domain::NfsPath::parse(r.path.as_str()).as_ref() == Ok(&r.path),
            || "an edit names a path that is not in its normal form".into(),
        )?;
        let text =
            String::from_utf8(r.content.to_vec()).map_err(|_| "edit content is not text".to_owned())?;
        let mut body = json!({ "path": r.path.as_str(), "expected": r.expected, "content": text });
        if let Some(eol) = r.change_eol {
            body["change_eol"] = json!(eol.as_str());
        }
        let again = edit_request(swimlane(), &serde_json::to_vec(&body).unwrap())
            .map_err(|e| format!("an accepted edit is refused when written canonically: {e}"))?;
        ensure(again == r, || "an edit changes in a round trip".into())?;
    }
    if let Ok(r) = revert_request(swimlane(), data) {
        ensure(
            domain::NfsPath::parse(r.path.as_str()).as_ref() == Ok(&r.path),
            || "a revert names a path that is not in its normal form".into(),
        )?;
        let mut body = json!({ "path": r.path.as_str(), "expected": r.expected, "to": r.to.to_string() });
        if let Some(eol) = r.change_eol {
            body["change_eol"] = json!(eol.as_str());
        }
        let again = revert_request(swimlane(), &serde_json::to_vec(&body).unwrap())
            .map_err(|e| format!("an accepted revert is refused when written canonically: {e}"))?;
        ensure(again == r, || "a revert changes in a round trip".into())?;
    }
    if let Ok(r) = restart_request(swimlane(), data) {
        let body = json!({ "namespace": r.service.namespace(), "name": r.service.name() });
        let again = restart_request(swimlane(), &serde_json::to_vec(&body).unwrap())
            .map_err(|e| format!("an accepted restart is refused when written canonically: {e}"))?;
        ensure(again == r, || "a restart changes in a round trip".into())?;
    }
    if let Some((path, expected, preview)) = delete_lines(data) {
        if let Ok(r) = delete_request(swimlane(), &path, &expected, preview.as_deref()) {
            let again = delete_request(
                swimlane(),
                r.path.as_str(),
                &r.expected.to_string(),
                r.preview.as_ref().map(domain::PreviewId::as_str),
            )
            .map_err(|e| format!("an accepted delete is refused when written canonically: {e}"))?;
            ensure(again == r, || "a delete changes in a round trip".into())?;
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------------------- multipart

/// The input is the content type, a line feed, and the body.
fn split_form(data: &[u8]) -> (String, Bytes) {
    match data.iter().position(|&b| b == b'\n') {
        Some(i) => (
            String::from_utf8_lossy(&data[..i]).into_owned(),
            Bytes::copy_from_slice(&data[i + 1..]),
        ),
        None => (String::from_utf8_lossy(data).into_owned(), Bytes::new()),
    }
}

/// A form in its canonical shape, with a boundary that none of the values contains.
fn encode(fields: &[(&str, Vec<u8>)]) -> (String, Bytes) {
    let mut n = 0_u32;
    let boundary = loop {
        let candidate = format!("lkFuzz{n}");
        let marker = format!("--{candidate}");
        if !fields
            .iter()
            .any(|(_, v)| v.windows(marker.len()).any(|w| w == marker.as_bytes()))
        {
            break candidate;
        }
        n += 1;
    };
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        Bytes::from(body),
    )
}

fn expected_fields(expected: &Expected) -> Vec<(&'static str, Vec<u8>)> {
    match expected {
        Expected::Absent => vec![("expected_state", b"absent".to_vec())],
        Expected::Hash { hash } => vec![
            ("expected_state", b"hash".to_vec()),
            ("expected_hash", hash.to_string().into_bytes()),
        ],
    }
}

pub fn upload_multipart_accepted(data: &[u8]) -> bool {
    let (content_type, body) = split_form(data);
    upload_request(swimlane(), &content_type, &body, MAX_FILE).is_ok()
        || preview_request(swimlane(), &content_type, &body, MAX_FILE).is_ok()
}

pub fn writes_upload_multipart(data: &[u8]) -> Result<(), String> {
    let (content_type, body) = split_form(data);
    if let Ok(r) = upload_request(swimlane(), &content_type, &body, MAX_FILE) {
        ensure(r.content.len() <= MAX_FILE, || {
            "an upload holds a file over the limit".into()
        })?;
        ensure(r.content.len() <= body.len(), || {
            "an upload holds more than it was sent".into()
        })?;
        ensure(
            domain::NfsPath::parse(r.path.as_str()).as_ref() == Ok(&r.path),
            || "an upload names a path that is not in its normal form".into(),
        )?;
        let mut fields = vec![("path", r.path.as_str().as_bytes().to_vec())];
        fields.extend(expected_fields(&r.expected));
        if let Some(p) = &r.preview {
            fields.push(("preview_id", p.as_str().as_bytes().to_vec()));
        }
        fields.push(("file", r.content.to_vec()));
        let (ct, canonical) = encode(&fields);
        let again = upload_request(swimlane(), &ct, &canonical, MAX_FILE)
            .map_err(|e| format!("an accepted upload is refused when written canonically: {e}"))?;
        ensure(again == r, || "an upload changes in a round trip".into())?;
    }
    if let Ok(r) = preview_request(swimlane(), &content_type, &body, MAX_FILE) {
        let mut fields: Vec<(&str, Vec<u8>)> = Vec::new();
        match &r.change {
            PreviewChange::Upload { expected, content } => {
                ensure(content.len() <= MAX_FILE, || {
                    "a preview holds a file over the limit".into()
                })?;
                fields.push(("action", b"upload".to_vec()));
                fields.push(("path", r.path.as_str().as_bytes().to_vec()));
                fields.extend(expected_fields(expected));
                fields.push(("file", content.to_vec()));
            }
            PreviewChange::Delete { expected } => {
                fields.push(("action", b"delete".to_vec()));
                fields.push(("path", r.path.as_str().as_bytes().to_vec()));
                fields.extend(expected_fields(&Expected::Hash { hash: *expected }));
            }
        }
        let (ct, canonical) = encode(&fields);
        let again = preview_request(swimlane(), &ct, &canonical, MAX_FILE)
            .map_err(|e| format!("an accepted preview is refused when written canonically: {e}"))?;
        ensure(again == r, || "a preview changes in a round trip".into())?;
    }
    Ok(())
}

// --------------------------------------------------------------------------------------------------------- preview id

pub fn preview_id_accepted(data: &[u8]) -> bool {
    fuzz_hooks::accepts(PREVIEW_KEY, &String::from_utf8_lossy(data), NOW_MS)
}

pub fn writes_preview_id(data: &[u8]) -> Result<(), String> {
    let token = String::from_utf8_lossy(data).into_owned();
    // The service's own token is always accepted now, and not a second after its ten minutes.
    let own = fuzz_hooks::issue_token(PREVIEW_KEY, NOW_MS).ok_or("no token could be issued")?;
    ensure(fuzz_hooks::accepts(PREVIEW_KEY, &own, NOW_MS), || {
        "the service refuses its own token".into()
    })?;
    ensure(fuzz_hooks::accepts(PREVIEW_KEY, &own, NOW_MS + 600_000), || {
        "the service refuses its own token at ten minutes".into()
    })?;
    ensure(!fuzz_hooks::accepts(PREVIEW_KEY, &own, NOW_MS + 601_000), || {
        "the service accepts a token after ten minutes".into()
    })?;

    if fuzz_hooks::accepts(PREVIEW_KEY, &token, NOW_MS) {
        let secs = fuzz_hooks::issued_secs(&token).ok_or("an accepted token does not name a time")?;
        let issued_ms = i64::try_from(secs).map_err(|_| "time out of range".to_owned())? * 1000;
        ensure(
            issued_ms <= NOW_MS + 30_000 && NOW_MS - issued_ms <= 600_000,
            || "an accepted token is outside the window".into(),
        )?;
        // The id for a request and a second is one string: no other text is accepted for it.
        ensure(
            fuzz_hooks::issue_token(PREVIEW_KEY, issued_ms).as_deref() == Some(token.as_str()),
            || "an accepted token is not the one the service issues for its second".into(),
        )?;
        // And only with the key: a token is useless under any other.
        ensure(
            !fuzz_hooks::accepts(b"another-key-another-key-another-ke", &token, NOW_MS),
            || "a token is accepted under another key".into(),
        )?;
    }
    Ok(())
}

/// The token the service issues for the fixed request at `now_ms`, for the generator of the seeds.
pub fn fuzz_hooks_issue(now_ms: i64) -> String {
    fuzz_hooks::issue_token(PREVIEW_KEY, now_ms).unwrap()
}
