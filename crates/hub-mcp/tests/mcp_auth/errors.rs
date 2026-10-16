//! S21, rule 3: the errors this crate answers with are a closed set with fixed text, and they mean what the hub's own guard means.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use domain::{Role, User, UserId};
use http::request::Parts;
use hub_mcp::McpError;
use hub_mcp::auth::pat::TokenError;
use hub_mcp::auth::store::{LabelError, StoreError};
use hub_platform::{GuardedRouter, SessionAuth};
use ports::AuthError;
use tower::ServiceExt;

const ALL: [McpError; 11] = [
    McpError::Unauthenticated,
    McpError::Forbidden,
    McpError::AuthUnavailable,
    McpError::NotFound,
    McpError::InvalidRequest,
    McpError::PayloadTooLarge,
    McpError::UnsupportedMediaType,
    McpError::TokenLimit,
    McpError::TooManySessions,
    McpError::Unavailable,
    McpError::Internal,
];

#[test]
fn errors_have_stable_statuses_and_codes() {
    let table = [
        (McpError::Unauthenticated, 401, "unauthorized"),
        (McpError::Forbidden, 403, "forbidden"),
        (McpError::AuthUnavailable, 503, "unavailable"),
        (McpError::NotFound, 404, "not_found"),
        (McpError::InvalidRequest, 400, "bad_request"),
        (McpError::PayloadTooLarge, 413, "payload_too_large"),
        (McpError::UnsupportedMediaType, 415, "unsupported_media_type"),
        (McpError::TokenLimit, 409, "conflict"),
        (McpError::TooManySessions, 429, "too_many_requests"),
        (McpError::Unavailable, 503, "unavailable"),
        (McpError::Internal, 500, "internal"),
    ];
    assert_eq!(table.len(), ALL.len());
    for (error, status, code) in table {
        assert_eq!(error.status().as_u16(), status, "{error:?}");
        assert_eq!(error.code(), code, "{error:?}");
    }
}

#[test]
fn error_messages_are_fixed_text_not_input() {
    // Display is a literal for every variant: it holds no field, so it cannot hold input.
    let mut seen = std::collections::BTreeSet::new();
    for error in ALL {
        let text = error.to_string();
        assert!(
            !text.is_empty() && text.len() < 64 && !text.contains(['{', '}', '"']),
            "{text}"
        );
        seen.insert(text);
    }
    assert_eq!(seen.len(), ALL.len(), "every variant says something different");
    // The errors of the layers below are fixed text as well.
    for text in [
        TokenError::LimitReached.to_string(),
        TokenError::NotFound.to_string(),
        TokenError::Unavailable.to_string(),
        TokenError::Internal.to_string(),
        StoreError::LimitReached.to_string(),
        StoreError::Unavailable.to_string(),
        LabelError::Empty.to_string(),
        LabelError::TooLong.to_string(),
        LabelError::Control.to_string(),
        hub_mcp::auth::token::KeyError::Encoding.to_string(),
        hub_mcp::auth::token::RandomError.to_string(),
    ] {
        assert!(
            !text.is_empty() && text.len() < 80 && !text.contains(['{', '}', '"']),
            "{text}"
        );
    }
}

#[test]
fn token_errors_map_onto_mcp_errors() {
    assert_eq!(McpError::from(TokenError::LimitReached), McpError::TokenLimit);
    assert_eq!(McpError::from(TokenError::NotFound), McpError::NotFound);
    assert_eq!(McpError::from(TokenError::Unavailable), McpError::Unavailable);
    assert_eq!(McpError::from(TokenError::Internal), McpError::Internal);
}

struct Fails(AuthError);

#[async_trait]
impl SessionAuth for Fails {
    async fn authenticate(&self, _parts: &Parts) -> Result<User, AuthError> {
        Err(self.0)
    }

    async fn still_valid(&self, _user: &UserId) -> bool {
        false
    }
}

#[tokio::test]
async fn auth_errors_map_to_the_statuses_the_hub_guard_gives() {
    // The gate answers a failed authentication as the hub's own route guard does, so one user gets one verdict on both doors.
    for error in [
        AuthError::InvalidToken,
        AuthError::Expired,
        AuthError::WrongAudience,
        AuthError::UnknownUser,
        AuthError::Pending,
        AuthError::Disabled,
        AuthError::InsufficientRole,
        AuthError::Unavailable,
    ] {
        let (router, _) = GuardedRouter::new(Arc::new(Fails(error)), None)
            .route(&http::Method::GET, "/x", Role::Viewer, || async { "ok" })
            .into_parts()
            .unwrap();
        let response = router
            .oneshot(Request::builder().uri("/x").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), McpError::from(error).status(), "{error:?}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json",
            "{error:?}"
        );
    }
    assert_eq!(McpError::Unauthenticated.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn write_outcomes_map_to_results_and_errors() {
    // A13: what the write service says is a result of the contract, or one of the contract's errors.
    use std::num::NonZeroU32;

    use domain::{AuditId, IssueKind, ProposalId, RejectReason, ValidationIssue, WriteOutcome};
    use hub_mcp::confirm::map_outcome;
    use hub_mcp::tools::Code;
    use jsonschema::Validator;
    use serde_json::{Value, json};

    use crate::fix::{hash_of, st};
    use crate::tools_rig::{contract, error_schema_problems};

    let validator: Validator = jsonschema::validator_for(&Value::Object(
        (**contract().tool("apply_change").unwrap().result()).clone(),
    ))
    .unwrap();
    let in_the_envelope = |result: &Value| {
        let envelope = json!({"summary": "s", "data": {"result": result}, "truncated": false, "next": null, "web_link": null});
        let problems: Vec<String> = validator.iter_errors(&envelope).map(|e| e.to_string()).collect();
        assert!(problems.is_empty(), "{result}: {problems:?}");
    };
    let issue = ValidationIssue {
        kind: IssueKind::DuplicateKey,
        line: NonZeroU32::new(9),
        rule: None,
    };

    // The results.
    let applied = map_outcome(&WriteOutcome::Applied {
        hash: Some(hash_of(b"x")),
        audit_id: AuditId::from(7),
        warnings: Vec::new(),
    })
    .unwrap();
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(applied["hash"], hash_of(b"x").to_string());
    assert_eq!(applied["audit_id"], 7);
    assert_eq!(applied["proposal_id"], Value::Null);
    assert!(applied.get("warnings").is_none(), "absent when there are none");
    in_the_envelope(&applied);

    let with_warnings = map_outcome(&WriteOutcome::Applied {
        hash: None,
        audit_id: AuditId::from(8),
        warnings: vec![issue.clone(); 3],
    })
    .unwrap();
    assert_eq!(with_warnings["hash"], Value::Null);
    assert_eq!(with_warnings["warnings"].as_array().unwrap().len(), 3);
    in_the_envelope(&with_warnings);

    let proposal = map_outcome(&WriteOutcome::ProposalCreated {
        id: ProposalId::from(3),
    })
    .unwrap();
    assert_eq!(proposal["outcome"], "proposal_created");
    assert_eq!(proposal["proposal_id"], 3);
    assert_eq!(proposal["audit_id"], Value::Null);
    in_the_envelope(&proposal);

    // The errors.
    let error_of = |outcome: WriteOutcome| map_outcome(&outcome).unwrap_err();
    let conflict = error_of(WriteOutcome::Conflict {
        current: Some(hash_of(b"now")),
    });
    assert_eq!(conflict.code, Code::Conflict);
    assert_eq!(conflict.current_hash, Some(hash_of(b"now")));
    assert_eq!(
        error_of(WriteOutcome::Conflict { current: None }).current_hash,
        None
    );
    assert_eq!(error_of(WriteOutcome::Locked { until: None }).code, Code::Locked);

    for (reason, code) in [
        (RejectReason::Denied, Code::Unprocessable),
        (RejectReason::InvalidContent, Code::Unprocessable),
        (RejectReason::SecretDetected, Code::Unprocessable),
        (RejectReason::ParseError, Code::Unprocessable),
        (RejectReason::TooLarge, Code::PayloadTooLarge),
        (RejectReason::UnknownSwimlane, Code::NotFound),
        (RejectReason::AgentUnavailable, Code::Unavailable),
        (RejectReason::NotAllowed, Code::Forbidden),
    ] {
        let error = error_of(WriteOutcome::Rejected {
            reason,
            issues: vec![ValidationIssue {
                kind: IssueKind::SecretMatch,
                line: NonZeroU32::new(4),
                rule: Some(st("github_token")),
            }],
        });
        assert_eq!(error.code, code, "{reason:?}");
        if code == Code::Unprocessable {
            // The reason and the places travel; the text of the lines never does (the issue holds a rule name and a line number).
            assert_eq!(error.reason, Some(reason));
            assert_eq!(error.issues.len(), 1);
        } else {
            assert_eq!(error.reason, None, "{reason:?}");
        }
        let (rendered, message) = error.render("req-1");
        assert!(
            error_schema_problems(&rendered).is_empty(),
            "{reason:?}: {rendered}"
        );
        assert!(!message.is_empty());
    }
    // Only what is retryable says so.
    assert!(error_of(WriteOutcome::Locked { until: None }).code.retryable());
    assert!(
        !error_of(WriteOutcome::Conflict { current: None })
            .code
            .retryable()
    );
}
