//! The properties the write path's fuzz targets assert (S11, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/hub-writes/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify` runs) check
//! exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a panic, which libFuzzer
//! reports as a crash. Input that is rightly refused is not a violation: the checks look at what was accepted.
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `writes_request_json` | bytes read as the JSON body of an edit, a revert, a restart, a draft, an approval and a rejection, and as the lines `path`, `expected`, `preview_id` of a delete and `status`, `swimlane`, `cursor`, `limit` of a list of proposals | reading never panics; an accepted request names a valid path and typed hashes, holds no more content than the input had, and survives a round trip through its canonical JSON (or lines) unchanged |
//! | `writes_upload_multipart` | a content type, a line end, and a multipart body, read as an upload form and as a preview form | reading never panics; an accepted request has a valid path, a file within the limit, an `Expected` that agrees with its fields, and survives a round trip through a canonical form unchanged |
//! | `writes_github_token` | bytes read as the body of `PUT /me/github-token`, and the same bytes as the text of a token | reading never panics; an accepted body holds a token of one to 512 bytes and survives a round trip through its canonical JSON unchanged; the shape check accepts only `github_pat_` followed by letters, digits and underscores within 512 bytes, never calls a fine-grained prefix classic, and its errors are the fixed words, none of which holds the input |
//! | `writes_github_responses` | bytes read as the answer of `GET /user`, as the answer of `GET /repos/{o}/{r}`, and as the value of the expiry header | reading never panics; an accepted login is valid and is the one an independent JSON read finds; `push` is `true` only when an independent read finds exactly `permissions.push == true`; every error is `Unexpected`; an accepted expiry lies in 2000 to 2200 and survives being written out and read again |
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
use hub_writes::github::api::{
    GithubError, MAX_BODY_BYTES, parse_push_permission, parse_user, status_error, token_expiry,
};
use hub_writes::github::types::{
    FINE_GRAINED_PREFIX, GithubLogin, MAX_TOKEN_LEN, TokenShapeError, check_token_shape,
};
use hub_writes::routes::wire::{
    approve_comment, delete_request, draft_request, edit_request, github_token_request, preview_request,
    proposal_id, proposal_query, reject_reason, restart_request, revert_request, upload_request,
};
use serde_json::json;

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Whether the input is one the target's reader accepts, for the names of the seeds.
pub type Accepts = fn(&[u8]) -> bool;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory), its check and its reader.
pub const TARGETS: [(&str, Check, Accepts); 5] = [
    ("writes_request_json", writes_request_json, request_json_accepted),
    (
        "writes_upload_multipart",
        writes_upload_multipart,
        upload_multipart_accepted,
    ),
    ("writes_preview_id", writes_preview_id, preview_id_accepted),
    ("writes_github_token", writes_github_token, github_token_accepted),
    (
        "writes_github_responses",
        writes_github_responses,
        github_responses_accepted,
    ),
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

/// The list of proposals reads the input as four lines: the status, the swimlane, the cursor and the limit. A line may be empty.
fn query_lines(data: &[u8]) -> (Option<String>, Option<String>, Option<String>, Option<u32>, bool) {
    let text = String::from_utf8_lossy(data).into_owned();
    let mut lines = text
        .split('\n')
        .map(|l| if l.is_empty() { None } else { Some(l.to_owned()) });
    let (status, swimlane, cursor) = (
        lines.next().flatten(),
        lines.next().flatten(),
        lines.next().flatten(),
    );
    let limit_line = lines.next().flatten();
    let limit = limit_line.as_deref().and_then(|l| l.parse::<u32>().ok());
    // A limit that is not a number is not a query at all (the route refuses it before the reader).
    let wellformed = limit_line.is_none() || limit.is_some();
    (status, swimlane, cursor, limit, wellformed)
}

pub fn request_json_accepted(data: &[u8]) -> bool {
    edit_request(swimlane(), data).is_ok()
        || revert_request(swimlane(), data).is_ok()
        || restart_request(swimlane(), data).is_ok()
        || draft_request(data).is_ok()
        // An approval may have no body at all; the empty input is not a seed of "accepted".
        || (!data.is_empty() && approve_comment(data).is_ok())
        || reject_reason(data).is_ok()
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
    if let Ok(r) = draft_request(data) {
        ensure(r.content.len() <= data.len(), || {
            "a draft holds more content than it was sent".into()
        })?;
        ensure(
            domain::NfsPath::parse(r.path.as_str()).as_ref() == Ok(&r.path),
            || "a draft names a path that is not in its normal form".into(),
        )?;
        let text =
            String::from_utf8(r.content.to_vec()).map_err(|_| "draft content is not text".to_owned())?;
        let body = json!({
            "swimlane": r.swimlane.as_str(),
            "path": r.path.as_str(),
            "expected": r.expected,
            "content": text,
        });
        let again = draft_request(&serde_json::to_vec(&body).unwrap())
            .map_err(|e| format!("an accepted draft is refused when written canonically: {e}"))?;
        ensure(again == r, || "a draft changes in a round trip".into())?;
    }
    if !data.is_empty() {
        if let Ok(comment) = approve_comment(data) {
            if let Some(c) = &comment {
                ensure(
                    c.as_str().len() <= 256 && !c.as_str().chars().any(char::is_control),
                    || "an approval comment is not short plain text".into(),
                )?;
            }
            let body = json!({ "comment": comment.as_ref().map(domain::ShortText::as_str) });
            let again = approve_comment(&serde_json::to_vec(&body).unwrap())
                .map_err(|e| format!("an accepted approval is refused when written canonically: {e}"))?;
            ensure(again == comment, || "an approval changes in a round trip".into())?;
        }
    }
    if let Ok(reason) = reject_reason(data) {
        ensure(!reason.as_str().trim().is_empty(), || {
            "a rejection has a blank reason".into()
        })?;
        let body = json!({ "reason": reason.as_str() });
        let again = reject_reason(&serde_json::to_vec(&body).unwrap())
            .map_err(|e| format!("an accepted rejection is refused when written canonically: {e}"))?;
        ensure(again == reason, || "a rejection changes in a round trip".into())?;
    }
    let (status, lane, cursor, limit, wellformed) = query_lines(data);
    if wellformed {
        if let Ok(q) = proposal_query(status.as_deref(), lane.as_deref(), cursor.as_deref(), limit) {
            ensure((1..=500).contains(&q.page.limit()), || {
                "a list asks for a page outside 1 to 500".into()
            })?;
            ensure(
                q.page.cursor().map(domain::Cursor::as_str) == cursor.as_deref(),
                || "a list changes its cursor".into(),
            )?;
            ensure(
                q.status.map(domain::ProposalStatus::as_str) == status.as_deref(),
                || "a list changes its status".into(),
            )?;
            ensure(
                q.swimlane.as_ref().map(SwimlaneId::as_str) == lane.as_deref(),
                || "a list changes its swimlane".into(),
            )?;
        }
    }
    let first_line = String::from_utf8_lossy(data)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned();
    if let Ok(id) = proposal_id(&first_line) {
        ensure(id.get() >= 1, || "a proposal id below one".into())?;
        ensure(first_line.bytes().all(|b| b.is_ascii_digit()), || {
            "a proposal id with a sign or a space".into()
        })?;
        ensure(proposal_id(&id.to_string()) == Ok(id), || {
            "a proposal id changes in a round trip".into()
        })?;
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

// ------------------------------------------------------------------------------------------------------- the GitHub token

pub fn github_token_accepted(data: &[u8]) -> bool {
    github_token_request(data).is_ok()
}

/// The words a refused shape is allowed to say: fixed, none of them holds the input.
const SHAPE_WORDS: [&str; 5] = [
    "the token is empty",
    "the token is too long",
    "classic tokens are not accepted",
    "the token is not a fine-grained token",
    "the token holds a character a token cannot hold",
];

fn shape_checks(text: &str) -> Result<(), String> {
    match check_token_shape(text) {
        Ok(()) => {
            ensure(text.starts_with(FINE_GRAINED_PREFIX), || {
                "a token without the fine-grained prefix is accepted".into()
            })?;
            ensure(text.len() <= MAX_TOKEN_LEN, || {
                "a token over the limit is accepted".into()
            })?;
            ensure(
                text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                || "a token with a character outside letters, digits and underscore is accepted".into(),
            )?;
            ensure(text.len() >= FINE_GRAINED_PREFIX.len() + 20, || {
                "a token too short to be one is accepted".into()
            })?;
        }
        Err(e) => {
            let shown = format!("{e}");
            ensure(SHAPE_WORDS.contains(&shown.as_str()), || {
                "a shape error says something other than its fixed words".into()
            })?;
            // A token with the fine-grained prefix is never told it is a classic one, and a classic one is never accepted.
            if text.starts_with(FINE_GRAINED_PREFIX) {
                ensure(e != TokenShapeError::Classic, || {
                    "a fine-grained prefix is called classic".into()
                })?;
            }
        }
    }
    Ok(())
}

pub fn writes_github_token(data: &[u8]) -> Result<(), String> {
    // The body of the request.
    if let Ok(token) = github_token_request(data) {
        let text = token.expose();
        ensure(!text.is_empty() && text.len() <= MAX_TOKEN_LEN, || {
            "an accepted body holds a token outside one to 512 bytes".into()
        })?;
        ensure(text.len() <= data.len(), || {
            "an accepted token is longer than the body".into()
        })?;
        let canonical = serde_json::to_vec(&json!({ "token": text })).map_err(|e| e.to_string())?;
        let again = github_token_request(&canonical)
            .map_err(|_| "an accepted body is refused when written canonically".to_owned())?;
        ensure(again.expose() == text, || {
            "a token changes in a round trip".into()
        })?;
        shape_checks(text)?;
    }
    // The same bytes as the text of a token.
    shape_checks(&String::from_utf8_lossy(data))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------- GitHub's answers

pub fn github_responses_accepted(data: &[u8]) -> bool {
    parse_user(data).is_ok() || parse_push_permission(data).is_ok() || token_expiry(Some(data)).is_some()
}

/// Days since 1970-01-01 to a civil date: the inverse of the parser's own conversion, written again here (Hinnant), so the check
/// does not trust the code it checks.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn writes_github_responses(data: &[u8]) -> Result<(), String> {
    let independent: Option<serde_json::Value> = serde_json::from_slice(data).ok();

    match parse_user(data) {
        Ok(login) => {
            ensure(data.len() <= MAX_BODY_BYTES, || {
                "a body over the cap is read".into()
            })?;
            ensure(GithubLogin::parse(login.as_str()).as_ref() == Ok(&login), || {
                "an accepted login is not a valid one".into()
            })?;
            ensure(login.as_str().len() <= 39, || {
                "an accepted login is too long".into()
            })?;
            let found = independent
                .as_ref()
                .and_then(|v| v.get("login"))
                .and_then(serde_json::Value::as_str);
            ensure(found == Some(login.as_str()), || {
                "the login is not the one an independent read finds".into()
            })?;
        }
        Err(e) => ensure(e == GithubError::Unexpected, || {
            "a login error is not 'unexpected'".into()
        })?,
    }

    match parse_push_permission(data) {
        Ok(push) => {
            ensure(data.len() <= MAX_BODY_BYTES, || {
                "a body over the cap is read".into()
            })?;
            let found = independent
                .as_ref()
                .and_then(|v| v.get("permissions"))
                .and_then(|p| p.get("push"))
                .and_then(serde_json::Value::as_bool);
            ensure(push == (found == Some(true)), || {
                "push is not what an independent read finds".into()
            })?;
        }
        Err(e) => ensure(e == GithubError::Unexpected, || {
            "a repository error is not 'unexpected'".into()
        })?,
    }

    if let Some(at) = token_expiry(Some(data)) {
        let ms = at.unix_millis();
        ensure((946_684_800_000..=7_258_118_400_000).contains(&ms), || {
            "an accepted expiry is outside 2000 to 2200".into()
        })?;
        ensure(ms % 1_000 == 0, || {
            "an accepted expiry has a fraction of a second".into()
        })?;
        let secs = ms / 1_000;
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let rem = secs.rem_euclid(86_400);
        let written = format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
            rem / 3_600,
            rem % 3_600 / 60,
            rem % 60
        );
        ensure(
            token_expiry(Some(written.as_bytes())).map(domain::Timestamp::unix_millis) == Some(ms),
            || "an accepted expiry changes when written out and read again".into(),
        )?;
        // And the text it was read from was that time, with nothing else around it but spaces.
        let text = std::str::from_utf8(data).map_err(|_| "a non-text expiry is accepted".to_owned())?;
        ensure(text.trim() == written, || {
            "the expiry text is not the time it names".into()
        })?;
    }

    // The status table is total: any status and flag give an answer, and only a 200 gives none.
    for chunk in data.chunks(3).take(64) {
        let status = u16::from(chunk[0]) << 8 | u16::from(*chunk.get(1).unwrap_or(&0));
        let limited = chunk.get(2).is_some_and(|b| b & 1 == 1);
        let got = status_error(status, limited);
        ensure(got.is_none() == (status == 200), || {
            "only a 200 is a success".into()
        })?;
        ensure(
            (got == Some(GithubError::Unavailable)) == (500..=599).contains(&status),
            || "only a 5xx is unavailable".into(),
        )?;
    }
    Ok(())
}
