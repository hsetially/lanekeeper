use async_trait::async_trait;
use domain::{DocPath, FeatureQuery, LineRange, LogicalFile, Role, UserStatus};

use super::sample::{active, user};
use crate::{DocError, DocSearch, MAX_DOC_HITS, MAX_DOC_PATTERN_BYTES, MAX_GREP_CONTEXT};

/// What the docs conformance suite needs from the test harness.
#[async_trait]
pub trait DocScenario: DocSearch {
    /// Add or replace a doc with this title and exact text.
    async fn seed(&self, path: DocPath, title: &str, text: &str);
}

/// - Pending, disabled and role-less users are `Forbidden` everywhere (S4).
/// - Limits are enforced: `limit` 1..=100, patterns at most 512 bytes, grep context at most 10.
/// - `search` finds matching docs, `read` returns exact lines of a doc (`NotFound` for unknown paths,
///   `BadRequest` for a range past the end), `grep` returns 1-based line numbers with bounded context.
pub async fn doc_search<D: DocScenario + ?Sized>(d: &D) {
    let viewer = active(1, Role::Viewer);
    let guide = DocPath::parse("/git/main/guides/alpha.md").unwrap();
    let other = DocPath::parse("/git/main/guides/beta.md").unwrap();
    d.seed(
        guide.clone(),
        "Alpha guide",
        "# Alpha\nline two mentions alpha twice: alpha\nline three\nsee tx-api/core.yml for the flag\n",
    )
    .await;
    d.seed(other, "Beta guide", "# Beta\nnothing relevant\n").await;

    for bad in [
        user(2, Some(Role::Admin), UserStatus::Pending),
        user(3, Some(Role::Admin), UserStatus::Disabled),
        user(4, None, UserStatus::Active),
    ] {
        assert_eq!(
            d.search(&bad, "alpha", 10).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(d.read(&bad, &guide, None).await.unwrap_err(), DocError::Forbidden);
        assert_eq!(
            d.grep(&bad, "alpha", false, 0).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.related(&bad, &core_yml()).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.feature_status(&bad, empty_query()).await.unwrap_err(),
            DocError::Forbidden
        );
    }

    let hits = d.search(&viewer, "alpha", 10).await.unwrap();
    assert_eq!(
        hits.first().map(|h| h.path.clone()),
        Some(guide.clone()),
        "best match first"
    );
    assert!(hits.len() <= 10);
    assert_eq!(d.search(&viewer, "alpha", 1).await.unwrap().len(), 1);
    assert_eq!(
        d.search(&viewer, "alpha", 0).await.unwrap_err(),
        DocError::BadRequest
    );
    assert_eq!(
        d.search(&viewer, "alpha", MAX_DOC_HITS + 1).await.unwrap_err(),
        DocError::BadRequest
    );
    let long = "a".repeat(MAX_DOC_PATTERN_BYTES + 1);
    assert_eq!(
        d.search(&viewer, &long, 10).await.unwrap_err(),
        DocError::BadRequest
    );

    let full = d.read(&viewer, &guide, None).await.unwrap();
    assert_eq!(full.total_lines, 4);
    assert!(full.text.starts_with("# Alpha"));
    let part = d
        .read(&viewer, &guide, Some(LineRange::new(2, 3).unwrap()))
        .await
        .unwrap();
    assert_eq!(part.text, "line two mentions alpha twice: alpha\nline three");
    assert_eq!(
        d.read(&viewer, &guide, Some(LineRange::new(50, 60).unwrap()))
            .await
            .unwrap_err(),
        DocError::BadRequest
    );
    let missing = DocPath::parse("/git/main/nope.md").unwrap();
    assert_eq!(
        d.read(&viewer, &missing, None).await.unwrap_err(),
        DocError::NotFound
    );

    let found = d.grep(&viewer, "line three", false, 1).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 3, "grep line numbers are 1-based");
    assert_eq!(found[0].before, ["line two mentions alpha twice: alpha"]);
    assert_eq!(found[0].after, ["see tx-api/core.yml for the flag"]);
    assert_eq!(
        d.grep(&viewer, "alpha", false, MAX_GREP_CONTEXT + 1)
            .await
            .unwrap_err(),
        DocError::BadRequest
    );
    assert_eq!(
        d.grep(&viewer, &long, false, 0).await.unwrap_err(),
        DocError::BadRequest
    );

    let related = d.related(&viewer, &core_yml()).await.unwrap();
    assert!(
        related.iter().any(|h| h.path == guide),
        "docs that mention the file"
    );

    d.feature_status(&viewer, empty_query())
        .await
        .expect("feature status");
}

fn empty_query() -> FeatureQuery {
    FeatureQuery {
        flag: None,
        file: None,
        channel: None,
        swimlanes: Vec::new(),
    }
}

fn core_yml() -> LogicalFile {
    LogicalFile::parse("tx-api/core.yml").unwrap()
}
