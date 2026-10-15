//! Reading a request into typed values (S11): pure functions from bytes to a [`domain`] request, so that the same code is what the
//! handlers run and what the fuzz targets run (S22).
//!
//! Nothing here echoes the input. A refusal is one of three fixed words; the handler turns it into the status and the fixed
//! problem body, so a body that holds a secret cannot come back in an error.

use bytes::Bytes;
use domain::{
    ChangeEol, ContentHash, Cursor, DeleteRequest, EditRequest, Expected, NfsPath, Page, PreviewChange,
    PreviewId, PreviewRequest, ProposalId, ProposalStatus, RestartRequest, RevertRequest, ServiceRef,
    ShortText, SwimlaneId, UploadRequest,
};
use serde::Deserialize;

use super::multipart::{Form, MultipartError, parse};
use crate::ProposalQuery;

/// Why a request was refused before the service saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// 400.
    #[error("the request is not valid")]
    BadRequest,
    /// 415.
    #[error("the media type is not accepted")]
    UnsupportedMediaType,
    /// 413.
    #[error("the request is larger than its limit")]
    TooLarge,
}

type Read<T> = Result<T, Refusal>;

// ---------------------------------------------------------------------------------------------------------- JSON bodies

/// The body of `PUT /files`. Unknown fields are refused: a typo is not silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditBody {
    path: NfsPath,
    expected: Expected,
    content: String,
    #[serde(default)]
    change_eol: Option<ChangeEol>,
}

/// The JSON body of an edit.
pub fn edit_request(swimlane: SwimlaneId, body: &[u8]) -> Read<EditRequest> {
    let body: EditBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
    Ok(EditRequest {
        swimlane,
        path: body.path,
        expected: body.expected,
        content: Bytes::from(body.content.into_bytes()),
        change_eol: body.change_eol,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevertBody {
    path: NfsPath,
    expected: Expected,
    to: ContentHash,
    #[serde(default)]
    change_eol: Option<ChangeEol>,
}

/// The JSON body of a revert.
pub fn revert_request(swimlane: SwimlaneId, body: &[u8]) -> Read<RevertRequest> {
    let body: RevertBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
    Ok(RevertRequest {
        swimlane,
        path: body.path,
        expected: body.expected,
        to: body.to,
        change_eol: body.change_eol,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartBody {
    namespace: String,
    name: String,
}

/// The JSON body of a restart: a `ServiceRef`.
pub fn restart_request(swimlane: SwimlaneId, body: &[u8]) -> Read<RestartRequest> {
    let body: RestartBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
    let service = ServiceRef::new(&body.namespace, &body.name).map_err(|_| Refusal::BadRequest)?;
    Ok(RestartRequest { swimlane, service })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftBody {
    swimlane: SwimlaneId,
    path: NfsPath,
    expected: Expected,
    content: String,
}

/// The JSON body of a draft: an edit that names its swimlane.
pub fn draft_request(body: &[u8]) -> Read<EditRequest> {
    let body: DraftBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
    Ok(EditRequest {
        swimlane: body.swimlane,
        path: body.path,
        expected: body.expected,
        content: Bytes::from(body.content.into_bytes()),
        change_eol: None,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveBody {
    #[serde(default)]
    comment: Option<ShortText>,
}

/// The body of an approval: nothing at all, or `{"comment": text or null}`.
pub fn approve_comment(body: &[u8]) -> Read<Option<ShortText>> {
    if body.is_empty() {
        return Ok(None);
    }
    let body: ApproveBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
    Ok(body.comment)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectBody {
    reason: ShortText,
}

/// The body of a rejection: `{"reason": text}`. A rejection says why, so the text is not empty.
pub fn reject_reason(body: &[u8]) -> Read<ShortText> {
    let body: RejectBody = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
    if body.reason.as_str().trim().is_empty() {
        return Err(Refusal::BadRequest);
    }
    Ok(body.reason)
}

/// The id of a proposal from its path segment: a positive integer.
pub fn proposal_id(text: &str) -> Read<ProposalId> {
    match text.parse::<i64>() {
        Ok(n) if n >= 1 && text.bytes().all(|b| b.is_ascii_digit()) => Ok(ProposalId::from(n)),
        _ => Err(Refusal::BadRequest),
    }
}

/// The query of a list of proposals: each value is parsed into its type, and the limit is the contract's (1 to 500, default 100).
pub fn proposal_query(
    status: Option<&str>,
    swimlane: Option<&str>,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Read<ProposalQuery> {
    let page = Page::new(
        limit.unwrap_or(Page::DEFAULT_LIMIT),
        cursor
            .map(Cursor::parse)
            .transpose()
            .map_err(|_| Refusal::BadRequest)?,
    )
    .map_err(|_| Refusal::BadRequest)?;
    Ok(ProposalQuery {
        status: status
            .map(str::parse::<ProposalStatus>)
            .transpose()
            .map_err(|_| Refusal::BadRequest)?,
        swimlane: swimlane
            .map(SwimlaneId::parse)
            .transpose()
            .map_err(|_| Refusal::BadRequest)?,
        page,
    })
}

/// A delete, from the three values of its query string: `path`, `expected` and the optional `preview_id`.
pub fn delete_request(
    swimlane: SwimlaneId,
    path: &str,
    expected: &str,
    preview_id: Option<&str>,
) -> Read<DeleteRequest> {
    Ok(DeleteRequest {
        swimlane,
        path: NfsPath::parse(path).map_err(|_| Refusal::BadRequest)?,
        expected: ContentHash::parse(expected).map_err(|_| Refusal::BadRequest)?,
        preview: preview_id
            .map(PreviewId::parse)
            .transpose()
            .map_err(|_| Refusal::BadRequest)?,
    })
}

// ------------------------------------------------------------------------------------------------------- multipart forms

fn form(content_type: &str, body: &Bytes, max_file: usize) -> Read<Form> {
    parse(content_type, body, max_file).map_err(|e| match e {
        MultipartError::NotMultipart => Refusal::UnsupportedMediaType,
        MultipartError::TooLarge => Refusal::TooLarge,
        MultipartError::Malformed | MultipartError::UnknownField | MultipartError::Duplicate => {
            Refusal::BadRequest
        }
    })
}

/// A text field that has to be there.
fn required<'a>(form: &'a Form, name: &str) -> Read<&'a str> {
    match form.text(name) {
        Some(Ok(text)) => Ok(text),
        _ => Err(Refusal::BadRequest),
    }
}

/// A text field that may be there.
fn optional<'a>(form: &'a Form, name: &str) -> Read<Option<&'a str>> {
    match form.text(name) {
        None => Ok(None),
        Some(Ok(text)) => Ok(Some(text)),
        Some(Err(_)) => Err(Refusal::BadRequest),
    }
}

/// `expected_state` and `expected_hash` as an `Expected`: `absent` has no hash, `hash` needs one.
fn expected(form: &Form) -> Read<Expected> {
    match (
        required(form, "expected_state")?,
        optional(form, "expected_hash")?,
    ) {
        ("absent", None) => Ok(Expected::Absent),
        ("hash", Some(hash)) => ContentHash::parse(hash)
            .map(|hash| Expected::Hash { hash })
            .map_err(|_| Refusal::BadRequest),
        _ => Err(Refusal::BadRequest),
    }
}

fn path(form: &Form) -> Read<NfsPath> {
    NfsPath::parse(required(form, "path")?).map_err(|_| Refusal::BadRequest)
}

/// The upload form: `path`, `expected_state`, `expected_hash`, `preview_id` and `file`. It has no `action`.
pub fn upload_request(
    swimlane: SwimlaneId,
    content_type: &str,
    body: &Bytes,
    max_file: usize,
) -> Read<UploadRequest> {
    let form = form(content_type, body, max_file)?;
    if form.has("action") {
        return Err(Refusal::BadRequest);
    }
    Ok(UploadRequest {
        swimlane,
        path: path(&form)?,
        expected: expected(&form)?,
        content: form.file().ok_or(Refusal::BadRequest)?,
        preview: optional(&form, "preview_id")?
            .map(PreviewId::parse)
            .transpose()
            .map_err(|_| Refusal::BadRequest)?,
    })
}

/// The preview form: `action` (`upload` or `delete`), `path`, `expected_state`, `expected_hash` and, for an upload, `file`.
/// It has no `preview_id`.
pub fn preview_request(
    swimlane: SwimlaneId,
    content_type: &str,
    body: &Bytes,
    max_file: usize,
) -> Read<PreviewRequest> {
    let form = form(content_type, body, max_file)?;
    if form.has("preview_id") {
        return Err(Refusal::BadRequest);
    }
    let change = match required(&form, "action")? {
        "upload" => PreviewChange::Upload {
            expected: expected(&form)?,
            content: form.file().ok_or(Refusal::BadRequest)?,
        },
        "delete" => {
            if form.has("file") {
                return Err(Refusal::BadRequest);
            }
            match expected(&form)? {
                Expected::Hash { hash } => PreviewChange::Delete { expected: hash },
                Expected::Absent => return Err(Refusal::BadRequest),
            }
        }
        _ => return Err(Refusal::BadRequest),
    };
    Ok(PreviewRequest {
        swimlane,
        path: path(&form)?,
        change,
    })
}
