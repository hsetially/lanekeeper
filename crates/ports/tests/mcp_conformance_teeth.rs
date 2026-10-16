//! The conformance suites have teeth for the methods of the `contract/07-mcp` change: an implementation that keeps the
//! `Unavailable` default of a new method fails, and so does one that gets the new behaviour wrong in a way the suite can see
//! (a user it should refuse, a folder in a list of files, a filter it ignores, a draft that is not the author's).
//!
//! Each wrapper forwards every method that existed before the change to a fake, and deviates in one new method.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    CompareQuery, CompareRef, Comparison, ContentSource, DocGrepHit, DocHit, DocPath, DocText, Draft,
    DraftId, DriftQuery, DriftState, EditRequest, EffectiveConfig, EffectiveQuery, FeatureQuery,
    FeatureStatus, FileContent, FileEntry, Finding, FindingsQuery, Grid, GridQuery, LineRange, LogicalFile,
    NfsPath, NotifyRequest, Page, Paged, PrRequest, PreviewRequest, RestartRequest, RevertRequest, Role,
    ServeRequest, ServedFile, ServiceState, SettingHit, SettingPath, SettingsQuery, SwimlaneId,
    SwimlaneSummary, TextQuery, TextResults, TreeComparison, UploadRequest, User, Version, WriteCtx,
    WriteOutcome,
};
use ports::conformance::{self, DocScenario, RegistryScenario, WriteScenario};
use ports::fakes::{FakeDocSearch, FakeRegistry, FakeWriteService};
use ports::{DocError, DocSearch, MAX_GREP_HITS, ReadError, RegistryRead, WriteError, WriteService};

fn file(path: &str, drift: DriftState, is_dir: bool) -> FileEntry {
    use domain::{ContentHash, FileClass, FileKind, FileRole};
    FileEntry {
        path: NfsPath::parse(path).unwrap(),
        is_dir,
        kind: FileKind::Base,
        class: FileClass::Structured,
        role: FileRole::PropertySource,
        size: 1,
        hash: (!is_dir).then(|| ContentHash::from_bytes([9; 32])),
        drift,
        format: None,
        content_withheld: false,
        attribution: None,
        severity: None,
        pickup_state: None,
        // A file names its logical file, which for a base file is its own path (CG2). The suite checks every drift entry.
        logical_file: (!is_dir).then(|| LogicalFile::parse(path).unwrap()),
        tenant: None,
    }
}

/// The impls of `RegistryRead` and `RegistryScenario` for a wrapper `$name` that has a field `inner: FakeRegistry`: the twelve
/// methods that existed before the change are forwarded, and `$new` is the rest of the impl (the methods of the change; a
/// wrapper that writes none of them keeps their default).
macro_rules! registry {
    ($name:ident { $($new:tt)* }) => {
        #[async_trait]
        impl RegistryRead for $name {
            async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
                self.inner.swimlanes(u).await
            }
            async fn tree(&self, u: &User, s: &SwimlaneId, prefix: &NfsPath, page: Page) -> Result<Paged<FileEntry>, ReadError> {
                self.inner.tree(u, s, prefix, page).await
            }
            async fn content(&self, u: &User, s: &SwimlaneId, p: &NfsPath, src: ContentSource) -> Result<FileContent, ReadError> {
                self.inner.content(u, s, p, src).await
            }
            async fn history(&self, u: &User, s: &SwimlaneId, p: &NfsPath, page: Page) -> Result<Paged<Version>, ReadError> {
                self.inner.history(u, s, p, page).await
            }
            async fn compare(&self, u: &User, l: CompareRef, r: CompareRef, p: Option<LogicalFile>) -> Result<Comparison, ReadError> {
                self.inner.compare(u, l, r, p).await
            }
            async fn compare_tree(&self, u: &User, l: CompareRef, r: CompareRef, page: Page) -> Result<TreeComparison, ReadError> {
                self.inner.compare_tree(u, l, r, page).await
            }
            async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
                self.inner.grid(u, q).await
            }
            async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
                self.inner.search_settings(u, q).await
            }
            async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
                self.inner.search_text(u, q).await
            }
            async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
                self.inner.findings(u, q).await
            }
            async fn effective(&self, u: &User, s: &SwimlaneId, f: &LogicalFile) -> Result<EffectiveConfig, ReadError> {
                self.inner.effective(u, s, f).await
            }
            async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
                self.inner.pending_restarts(u, s).await
            }
            $($new)*
        }

        #[async_trait]
        impl RegistryScenario for $name {
            async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
                FakeRegistry::seed(&self.inner, s, files);
            }
        }
    };
}

/// The same for `DocSearch` and `DocScenario`, over a field `inner: FakeDocSearch`.
macro_rules! docs {
    ($name:ident { $($new:tt)* }) => {
        #[async_trait]
        impl DocSearch for $name {
            async fn search(&self, u: &User, q: &str, limit: usize) -> Result<Vec<DocHit>, DocError> {
                self.inner.search(u, q, limit).await
            }
            async fn related(&self, u: &User, f: &LogicalFile) -> Result<Vec<DocHit>, DocError> {
                self.inner.related(u, f).await
            }
            async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError> {
                self.inner.read(u, path, lines).await
            }
            async fn grep(&self, u: &User, pattern: &str, regex: bool, context: u8) -> Result<Vec<DocGrepHit>, DocError> {
                self.inner.grep(u, pattern, regex, context).await
            }
            async fn feature_status(&self, u: &User, q: FeatureQuery) -> Result<FeatureStatus, DocError> {
                self.inner.feature_status(u, q).await
            }
            $($new)*
        }

        #[async_trait]
        impl DocScenario for $name {
            async fn seed(&self, path: DocPath, title: &str, text: &str) {
                FakeDocSearch::seed(&self.inner, path, title, text);
            }
        }
    };
}

/// A `WriteService` over a fake, the same way. `$extra` are fields of the wrapper, `$new` the methods it overrides.
macro_rules! writes {
    ($name:ident { $($new:tt)* }) => {
        struct $name {
            inner: FakeWriteService,
            /// Every draft this wrapper has seen proposed, whoever it belonged to.
            seen: Mutex<HashMap<DraftId, Draft>>,
            /// How many seconds the suite has moved the clock by.
            advanced: AtomicU64,
        }

        impl $name {
            fn new() -> Self {
                Self { inner: FakeWriteService::new(), seen: Mutex::new(HashMap::new()), advanced: AtomicU64::new(0) }
            }
        }

        #[async_trait]
        impl WriteService for $name {
            async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.edit(ctx, r).await
            }
            async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.upload(ctx, r).await
            }
            async fn delete(&self, ctx: &WriteCtx, r: domain::DeleteRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.delete(ctx, r).await
            }
            async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.revert(ctx, r).await
            }
            async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.restart(ctx, r).await
            }
            async fn raise_pr(&self, ctx: &WriteCtx, r: PrRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.raise_pr(ctx, r).await
            }
            async fn propose_draft(&self, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError> {
                let d = self.inner.propose_draft(ctx, r).await?;
                self.seen.lock().unwrap().insert(d.id.clone(), d.clone());
                Ok(d)
            }
            async fn apply_draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<WriteOutcome, WriteError> {
                self.inner.apply_draft(ctx, id).await
            }
            async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<domain::ChangePreview, WriteError> {
                self.inner.preview_effect(ctx, r).await
            }
            async fn notify(&self, ctx: &WriteCtx, r: NotifyRequest) -> Result<WriteOutcome, WriteError> {
                self.inner.notify(ctx, r).await
            }
            $($new)*
        }

        #[async_trait]
        impl WriteScenario for $name {
            async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
                FakeWriteService::seed(&self.inner, s, files);
            }
            async fn nfs_content(&self, s: &SwimlaneId, p: &NfsPath) -> Option<Bytes> {
                FakeWriteService::nfs_content(&self.inner, s, p)
            }
            async fn advance_clock(&self, secs: u64) {
                self.advanced.fetch_add(secs, Ordering::Relaxed);
                FakeWriteService::advance_clock(&self.inner, secs);
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------------------------
// RegistryRead

/// Forwards the twelve old methods and none of the new: they keep their default.
struct KeepsTheDefaults {
    inner: FakeRegistry,
}
registry!(KeepsTheDefaults {});

/// How a [`Deviating`] registry gets the new behaviour wrong. `Honest` gets it right.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deviation {
    Honest,
    AutoNotifyIsAlwaysOn,
    DriftForEveryone,
    DriftListsFolders,
    DriftListsFilesInSync,
    DriftIgnoresThePageLimit,
    DriftIgnoresTheState,
    DriftIgnoresThePrefix,
}

/// Forwards everything, the new methods too, and deviates in one.
struct Deviating {
    inner: FakeRegistry,
    how: Deviation,
}

impl Deviating {
    fn new(how: Deviation) -> Self {
        Self {
            inner: FakeRegistry::new(),
            how,
        }
    }
}

registry!(Deviating {
    async fn effective_for(&self, u: &User, s: &SwimlaneId, q: &EffectiveQuery) -> Result<EffectiveConfig, ReadError> {
        self.inner.effective_for(u, s, q).await
    }
    async fn compare_in(&self, u: &User, q: &CompareQuery) -> Result<Comparison, ReadError> {
        self.inner.compare_in(u, q).await
    }
    async fn compare_tree_under(&self, u: &User, l: CompareRef, r: CompareRef, prefix: &NfsPath, page: Page) -> Result<TreeComparison, ReadError> {
        self.inner.compare_tree_under(u, l, r, prefix, page).await
    }
    async fn drift(&self, u: &User, s: &SwimlaneId, q: &DriftQuery, page: Page) -> Result<Paged<FileEntry>, ReadError> {
        let honest = self.inner.drift(u, s, q, page).await;
        let listed = |files: Vec<FileEntry>| honest.clone().map(|_| Paged::last(files));
        match self.how {
            Deviation::DriftForEveryone => Ok(Paged::last(Vec::new())),
            Deviation::DriftListsFolders => listed(vec![file("a", DriftState::NfsAhead, true)]),
            Deviation::DriftListsFilesInSync => listed(vec![file("a/one.yml", DriftState::InSync, false)]),
            Deviation::DriftIgnoresThePageLimit => listed(vec![
                file("a/1.yml", DriftState::NfsAhead, false),
                file("a/2.yml", DriftState::NfsAhead, false),
                file("a/3.yml", DriftState::NfsAhead, false),
            ]),
            Deviation::DriftIgnoresTheState => listed(vec![file("a/one.yml", DriftState::Conflict, false)]),
            // Honest about the state, so that only the prefix is wrong.
            Deviation::DriftIgnoresThePrefix => {
                listed(vec![file("b/one.yml", q.state.unwrap_or(DriftState::NfsAhead), false)])
            }
            Deviation::Honest | Deviation::AutoNotifyIsAlwaysOn => honest,
        }
    }
    async fn auto_notify(&self, u: &User, s: &SwimlaneId) -> Result<bool, ReadError> {
        let honest = self.inner.auto_notify(u, s).await;
        if self.how == Deviation::AutoNotifyIsAlwaysOn { honest.map(|_| true) } else { honest }
    }
    async fn served(&self, u: &User, s: &SwimlaneId, r: &ServeRequest) -> Result<ServedFile, ReadError> {
        self.inner.served(u, s, r).await
    }
});

#[tokio::test(start_paused = true)]
async fn the_registry_suite_passes_a_wrapper_that_deviates_in_nothing() {
    // The wrappers fail for what they do wrong, not for being wrappers: forwarding the new methods too passes.
    conformance::registry_read(&Deviating::new(Deviation::Honest)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "left: Unavailable")]
async fn a_registry_that_keeps_the_defaults_fails() {
    conformance::registry_read(&KeepsTheDefaults {
        inner: FakeRegistry::new(),
    })
    .await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "auto-notify is off for a swimlane nobody switched on")]
async fn a_registry_whose_auto_notify_is_on_by_default_fails() {
    conformance::registry_read(&Deviating::new(Deviation::AutoNotifyIsAlwaysOn)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "unwrap_err")]
async fn a_registry_that_lists_drift_for_a_user_it_should_refuse_fails() {
    conformance::registry_read(&Deviating::new(Deviation::DriftForEveryone)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "drift lists files, not folders")]
async fn a_registry_that_lists_a_folder_as_drift_fails() {
    conformance::registry_read(&Deviating::new(Deviation::DriftListsFolders)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a file in sync is not drift")]
async fn a_registry_that_lists_a_file_in_sync_as_drift_fails() {
    conformance::registry_read(&Deviating::new(Deviation::DriftListsFilesInSync)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a page never exceeds its limit")]
async fn a_registry_whose_drift_ignores_the_page_limit_fails() {
    conformance::registry_read(&Deviating::new(Deviation::DriftIgnoresThePageLimit)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "only files in that state")]
async fn a_registry_whose_drift_ignores_the_state_fails() {
    conformance::registry_read(&Deviating::new(Deviation::DriftIgnoresTheState)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "only files below the prefix")]
async fn a_registry_whose_drift_ignores_the_prefix_fails() {
    conformance::registry_read(&Deviating::new(Deviation::DriftIgnoresThePrefix)).await;
}

// ---------------------------------------------------------------------------------------------------------------
// DocSearch

/// Forwards the five old methods and none of the new: they keep their default.
struct DocsKeepTheDefaults {
    inner: FakeDocSearch,
}
docs!(DocsKeepTheDefaults {});

#[derive(Clone, Copy, PartialEq, Eq)]
enum DocDeviation {
    Honest,
    RelatedIgnoresTheSetting,
    GrepIgnoresThePrefix,
    GrepIgnoresTheLimit,
}

/// Forwards everything, the new methods too, and deviates in one.
struct DeviatingDocs {
    inner: FakeDocSearch,
    how: DocDeviation,
}

impl DeviatingDocs {
    fn new(how: DocDeviation) -> Self {
        Self {
            inner: FakeDocSearch::new(),
            how,
        }
    }
}

docs!(DeviatingDocs {
    async fn related_to(&self, u: &User, f: &LogicalFile, setting: Option<&SettingPath>, limit: usize) -> Result<Vec<DocHit>, DocError> {
        let setting = if self.how == DocDeviation::RelatedIgnoresTheSetting { None } else { setting };
        self.inner.related_to(u, f, setting, limit).await
    }
    async fn grep_in(&self, u: &User, pattern: &str, regex: bool, context: u8, prefix: Option<&DocPath>, limit: usize) -> Result<Vec<DocGrepHit>, DocError> {
        let prefix = if self.how == DocDeviation::GrepIgnoresThePrefix { None } else { prefix };
        let limit = if self.how == DocDeviation::GrepIgnoresTheLimit { MAX_GREP_HITS } else { limit };
        self.inner.grep_in(u, pattern, regex, context, prefix, limit).await
    }
    async fn render(&self, u: &User, path: &DocPath) -> Result<domain::RenderedDoc, DocError> {
        self.inner.render(u, path).await
    }
});

#[tokio::test(start_paused = true)]
async fn the_doc_suite_passes_a_wrapper_that_deviates_in_nothing() {
    conformance::doc_search(&DeviatingDocs::new(DocDeviation::Honest)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "left: Unavailable")]
async fn a_doc_search_that_keeps_the_defaults_fails() {
    conformance::doc_search(&DocsKeepTheDefaults {
        inner: FakeDocSearch::new(),
    })
    .await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "only the doc that mentions the setting as well")]
async fn a_doc_search_whose_related_to_ignores_the_setting_fails() {
    conformance::doc_search(&DeviatingDocs::new(DocDeviation::RelatedIgnoresTheSetting)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "only the docs below the prefix")]
async fn a_doc_search_whose_grep_in_ignores_the_prefix_fails() {
    conformance::doc_search(&DeviatingDocs::new(DocDeviation::GrepIgnoresThePrefix)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "left: 2")]
async fn a_doc_search_whose_grep_in_ignores_the_limit_fails() {
    conformance::doc_search(&DeviatingDocs::new(DocDeviation::GrepIgnoresTheLimit)).await;
}

// ---------------------------------------------------------------------------------------------------------------
// WriteService

writes!(WritesKeepTheDefaults {});

writes!(AnEditorReadsAnyDraft {
    async fn draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<Draft, WriteError> {
        if !ctx.user.allows(Role::Editor) {
            return Err(WriteError::Forbidden);
        }
        self.seen.lock().unwrap().get(&id).cloned().ok_or(WriteError::NotFound)
    }
});

// Reads drafts correctly until the clock has moved a day (the fake's draft life), then answers `NotFound` for every draft.
writes!(ExpiredDraftsAreGone {
    async fn draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<Draft, WriteError> {
        let honest = self.inner.draft(ctx, id).await;
        if self.advanced.load(Ordering::Relaxed) >= 24 * 60 * 60 {
            honest.and(Err(WriteError::NotFound))
        } else {
            honest
        }
    }
});

#[tokio::test(start_paused = true)]
#[should_panic(expected = "an expired draft is still readable by its author")]
async fn a_write_service_that_forgets_an_expired_draft_fails() {
    conformance::write_service(&ExpiredDraftsAreGone::new()).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "Unavailable")]
async fn a_write_service_that_keeps_the_draft_default_fails() {
    conformance::write_service(&WritesKeepTheDefaults::new()).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "only the author reads a draft")]
async fn a_write_service_that_shows_a_draft_to_another_editor_fails() {
    conformance::write_service(&AnEditorReadsAnyDraft::new()).await;
}
