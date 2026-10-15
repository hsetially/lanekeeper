use async_trait::async_trait;
use domain::{DocPath, FeatureQuery, LineRange, LogicalFile, Role, SettingPath, UserStatus};

use super::sample::{active, user};
use crate::{DocError, DocSearch, MAX_DOC_HITS, MAX_DOC_PATTERN_BYTES, MAX_GREP_CONTEXT, MAX_GREP_HITS};

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
/// - `related_to` (`contract/07-mcp`) returns the docs that mention the file, and the setting too when one is given, at most
///   `limit` of them (1..=100). `grep_in` greps the docs under a path prefix only, whole path components, at most `limit` hits
///   (1..=500), with the limits of `grep`. An implementation that keeps the `Unavailable` default of these fails here.
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
    // A doc of another folder that mentions the same file and a setting the first one does not, and the word `mentions`.
    let ports = DocPath::parse("/git/main/reference/ports.md").unwrap();
    d.seed(
        ports.clone(),
        "Ports reference",
        "# Ports\nthis page mentions tx-api/core.yml and the setting server.port\n",
    )
    .await;

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
        assert_eq!(
            d.related_to(&bad, &core_yml(), None, 10).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.grep_in(&bad, "alpha", false, 0, None, 10).await.unwrap_err(),
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

    related_to_and_grep_in(d, &viewer, &guide, &ports).await;
}

/// `related_to` and `grep_in`: the setting, the limit and the prefix narrow the answer, and the limits are enforced.
async fn related_to_and_grep_in<D: DocScenario + ?Sized>(
    d: &D,
    viewer: &domain::User,
    guide: &DocPath,
    ports: &DocPath,
) {
    let paths = |hits: &[domain::DocHit]| hits.iter().map(|h| h.path.clone()).collect::<Vec<_>>();
    let setting = SettingPath::parse("server.port").unwrap();

    // Docs that mention the file; with a setting, only those that mention it too; at most `limit`.
    let both = paths(&d.related_to(viewer, &core_yml(), None, 10).await.unwrap());
    assert!(
        both.contains(guide) && both.contains(ports),
        "both docs mention the file: {both:?}"
    );
    let narrowed = paths(
        &d.related_to(viewer, &core_yml(), Some(&setting), 10)
            .await
            .unwrap(),
    );
    assert_eq!(
        narrowed,
        [ports.clone()],
        "only the doc that mentions the setting as well"
    );
    let unknown = SettingPath::parse("no.such.setting").unwrap();
    assert!(
        d.related_to(viewer, &core_yml(), Some(&unknown), 10)
            .await
            .unwrap()
            .is_empty(),
        "a setting no doc mentions is an empty answer, not an error"
    );
    assert_eq!(
        d.related_to(viewer, &core_yml(), None, 1).await.unwrap().len(),
        1,
        "at most `limit`"
    );
    for limit in [0, MAX_DOC_HITS + 1] {
        assert_eq!(
            d.related_to(viewer, &core_yml(), None, limit).await.unwrap_err(),
            DocError::BadRequest,
            "limit {limit}"
        );
    }
    assert!(
        d.related_to(viewer, &core_yml(), None, MAX_DOC_HITS)
            .await
            .is_ok()
    );

    // Grep under a prefix: a whole path component, the prefix itself or below it.
    let all = d.grep_in(viewer, "mentions", false, 0, None, 20).await.unwrap();
    let hit_paths = paths_of_grep(&all);
    assert!(
        hit_paths.contains(guide) && hit_paths.contains(ports),
        "both docs say `mentions`: {hit_paths:?}"
    );
    let guides = DocPath::parse("/git/main/guides").unwrap();
    let under = d
        .grep_in(viewer, "mentions", false, 0, Some(&guides), 20)
        .await
        .unwrap();
    assert_eq!(
        paths_of_grep(&under),
        [guide.clone()],
        "only the docs below the prefix"
    );
    let itself = d
        .grep_in(viewer, "mentions", false, 0, Some(guide), 20)
        .await
        .unwrap();
    assert_eq!(
        paths_of_grep(&itself),
        [guide.clone()],
        "the prefix may be a doc itself"
    );
    let partial = DocPath::parse("/git/main/guide").unwrap();
    assert!(
        d.grep_in(viewer, "mentions", false, 0, Some(&partial), 20)
            .await
            .unwrap()
            .is_empty(),
        "`guide` is not the folder `guides`"
    );
    let uploads = DocPath::parse("/uploads/none").unwrap();
    assert!(
        d.grep_in(viewer, "mentions", false, 0, Some(&uploads), 20)
            .await
            .unwrap()
            .is_empty(),
        "a prefix with no docs is an empty answer, not an error"
    );
    // Context and line numbers are `grep`'s.
    let ctx = d
        .grep_in(viewer, "line three", false, 1, Some(&guides), 20)
        .await
        .unwrap();
    assert_eq!(ctx.len(), 1);
    assert_eq!(ctx[0].line, 3, "1-based, as for `grep`");
    assert_eq!(ctx[0].before, ["line two mentions alpha twice: alpha"]);

    // At most `limit` hits, and the limits of `grep`.
    assert_eq!(
        d.grep_in(viewer, "mentions", false, 0, None, 1)
            .await
            .unwrap()
            .len(),
        1
    );
    for limit in [0, MAX_GREP_HITS + 1] {
        assert_eq!(
            d.grep_in(viewer, "mentions", false, 0, None, limit)
                .await
                .unwrap_err(),
            DocError::BadRequest,
            "limit {limit}"
        );
    }
    assert!(
        d.grep_in(viewer, "mentions", false, 0, None, MAX_GREP_HITS)
            .await
            .is_ok()
    );
    let long = "a".repeat(MAX_DOC_PATTERN_BYTES + 1);
    assert_eq!(
        d.grep_in(viewer, &long, false, 0, None, 10).await.unwrap_err(),
        DocError::BadRequest
    );
    assert_eq!(
        d.grep_in(viewer, "mentions", false, MAX_GREP_CONTEXT + 1, None, 10)
            .await
            .unwrap_err(),
        DocError::BadRequest
    );
}

fn paths_of_grep(hits: &[domain::DocGrepHit]) -> Vec<DocPath> {
    let mut seen: Vec<DocPath> = Vec::new();
    for h in hits {
        if !seen.contains(&h.path) {
            seen.push(h.path.clone());
        }
    }
    seen.sort();
    seen
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
