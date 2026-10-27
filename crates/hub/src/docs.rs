//! The document search the hub has until prompt 14 lands (plan 18, T6, G5; S14, S15).
//!
//! `hub-docsearch` is empty: nothing yet implements [`DocSearch`] for production. The MCP server needs one to start (its five docs
//! tools stand on it), and the hub never starts on a fake: a stand-in that answered with an empty list would tell a model that
//! there are no documents about a setting, which is a claim about the world and a false one. [`UnavailableDocs`] is the single named,
//! tested exception. Every method answers [`DocError::Unavailable`], the port's own word for a search that cannot be served, which
//! the MCP crate maps to its typed `unavailable` result (retryable, no data) and the REST edge to a `503`. It holds no document, no
//! index and no state, so it cannot return one by accident.
//!
//! **It says so, three ways.** The start logs a warning. `/readyz` stays `200` and its body says `ready (not available:
//! docs_search)`, because a hub without document search can still serve everything else, and a probe that gates on the status must
//! not take it out of rotation; a person who reads the body is told the truth. And `lanekeeper_docs_search_available` is `0` for
//! as long as this is what the hub runs, so an alert can ask for it.
//!
//! **Replacing it** is a few lines in `process.rs` when prompt 14 has a real search: the real type goes where this one is built, the
//! note and the gauge are not registered for it, and the source scan in `tests/release_build.rs` that allows this one
//! implementation is changed in the same commit.

use async_trait::async_trait;
use domain::{
    DocGrepHit, DocHit, DocPath, DocText, FeatureQuery, FeatureStatus, LineRange, LogicalFile, RenderedDoc,
    SettingPath, User,
};
use ports::{DocError, DocSearch};
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

/// A [`DocSearch`] that is not there. See the module's notes.
#[derive(Debug, Clone, Default)]
pub struct UnavailableDocs {
    available: Gauge,
}

impl UnavailableDocs {
    /// The name `/readyz` gives this feature in its note.
    pub const NOTE: &'static str = "docs_search";

    #[must_use]
    pub fn new() -> Self {
        // The gauge starts at its value: this type is never available.
        Self::default()
    }

    /// Register `lanekeeper_docs_search_available` (0: this hub has no document search).
    pub fn register_metrics(&self, registry: &mut Registry) {
        registry.register(
            "docs_search_available",
            "1 when the hub serves document search, 0 when this build has none (prompt 14)",
            self.available.clone(),
        );
    }
}

#[async_trait]
impl DocSearch for UnavailableDocs {
    async fn search(&self, _u: &User, _q: &str, _limit: usize) -> Result<Vec<DocHit>, DocError> {
        Err(DocError::Unavailable)
    }

    async fn related(&self, _u: &User, _f: &LogicalFile) -> Result<Vec<DocHit>, DocError> {
        Err(DocError::Unavailable)
    }

    async fn read(&self, _u: &User, _path: &DocPath, _lines: Option<LineRange>) -> Result<DocText, DocError> {
        Err(DocError::Unavailable)
    }

    async fn grep(
        &self,
        _u: &User,
        _pattern: &str,
        _regex: bool,
        _context: u8,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        Err(DocError::Unavailable)
    }

    async fn feature_status(&self, _u: &User, _q: FeatureQuery) -> Result<FeatureStatus, DocError> {
        Err(DocError::Unavailable)
    }

    // The three methods below have the same default in the trait. They are written out so that this type does not depend on it: a
    // wrapper of `DocSearch` must forward them (the trait's own note), and a default that changed would otherwise change this type.

    async fn related_to(
        &self,
        _u: &User,
        _f: &LogicalFile,
        _setting: Option<&SettingPath>,
        _limit: usize,
    ) -> Result<Vec<DocHit>, DocError> {
        Err(DocError::Unavailable)
    }

    async fn render(&self, _u: &User, _path: &DocPath) -> Result<RenderedDoc, DocError> {
        Err(DocError::Unavailable)
    }

    async fn grep_in(
        &self,
        _u: &User,
        _pattern: &str,
        _regex: bool,
        _context: u8,
        _prefix: Option<&DocPath>,
        _limit: usize,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        Err(DocError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use domain::{
        DocPath, FeatureQuery, LineRange, LogicalFile, Role, SettingPath, ShortText, User, UserId, UserStatus,
    };
    use ports::{DocError, DocSearch};
    use prometheus_client::encoding::text;
    use prometheus_client::registry::Registry;

    use super::*;

    fn user() -> User {
        User {
            id: UserId::new(
                domain::Guid::parse("11111111-2222-4333-8444-555555555555").unwrap(),
                domain::Guid::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
            ),
            email: ShortText::parse("alice@example.test").unwrap(),
            display_name: ShortText::parse("Alice").unwrap(),
            role: Some(Role::Admin),
            status: UserStatus::Active,
            requires_approval: false,
            github_login: None,
            last_seen_at: None,
        }
    }

    fn no_flag() -> FeatureQuery {
        FeatureQuery {
            flag: None,
            file: None,
            channel: None,
            swimlanes: Vec::new(),
        }
    }

    #[tokio::test]
    async fn every_method_says_unavailable_and_none_returns_data() {
        let docs = UnavailableDocs::new();
        let u = user();
        let path = DocPath::parse("/git/templates/common-docs/guide.md").unwrap();
        let file = LogicalFile::parse("tx-infinity-api/tx-infinity-core.yml").unwrap();
        let setting = SettingPath::parse("a.b").unwrap();
        // An empty success would look like "no documents match", which is a different statement from "there is no document search".
        assert_eq!(
            docs.search(&u, "ledger", 10).await.unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(docs.related(&u, &file).await.unwrap_err(), DocError::Unavailable);
        assert_eq!(
            docs.read(&u, &path, Some(LineRange::new(1, 5).unwrap()))
                .await
                .unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(
            docs.read(&u, &path, None).await.unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(
            docs.grep(&u, "x", false, 0).await.unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(
            docs.feature_status(&u, no_flag()).await.unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(
            docs.related_to(&u, &file, Some(&setting), 5).await.unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(docs.render(&u, &path).await.unwrap_err(), DocError::Unavailable);
        assert_eq!(
            docs.grep_in(&u, "x", true, 2, Some(&path), 10).await.unwrap_err(),
            DocError::Unavailable
        );
    }

    #[tokio::test]
    async fn a_request_that_is_not_valid_is_still_unavailable_not_a_bad_request() {
        // There is no search to hold the request against: the answer does not depend on the request at all, so it tells the caller
        // nothing about it (and a bad limit is not "fixed" by retrying with a good one).
        let docs = UnavailableDocs::new();
        assert_eq!(
            docs.search(&user(), "", 0).await.unwrap_err(),
            DocError::Unavailable
        );
        assert_eq!(
            docs.search(&user(), &"x".repeat(100_000), 100_000)
                .await
                .unwrap_err(),
            DocError::Unavailable
        );
    }

    #[test]
    fn the_gauge_says_zero_and_the_note_names_the_feature() {
        let docs = UnavailableDocs::new();
        let mut registry = Registry::with_prefix("lanekeeper");
        docs.register_metrics(&mut registry);
        let mut out = String::new();
        text::encode(&mut out, &registry).unwrap();
        assert!(out.contains("lanekeeper_docs_search_available 0\n"), "{out}");
        assert_eq!(UnavailableDocs::NOTE, "docs_search");
    }
}
