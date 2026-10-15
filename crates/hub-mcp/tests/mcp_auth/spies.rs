//! The ports' fakes with a window into them: what was asked, a failure on demand, a delay on demand.
//!
//! The fakes answer; these also remember the questions, so a test can say that a tenant, a channel, a prefix or a page reached the
//! port as typed values, and that a write method was never called.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    ChangePreview, CompareQuery, CompareRef, Comparison, ContentSource, DeleteRequest, DocGrepHit, DocHit,
    DocPath, DocText, Draft, DraftId, DriftQuery, EditRequest, EffectiveConfig, EffectiveQuery, FeatureQuery,
    FeatureStatus, FileContent, FileEntry, Finding, FindingsQuery, Grid, GridQuery, LineRange, LogicalFile,
    NfsPath, NotifyRequest, Page, Paged, PrRequest, PreviewRequest, RestartRequest, RevertRequest,
    ServeRequest, ServedFile, ServiceState, SettingHit, SettingPath, SettingsQuery, SwimlaneId,
    SwimlaneSummary, TextQuery, TextResults, TreeComparison, UploadRequest, User, Version, WriteCtx,
    WriteOutcome,
};
use ports::fakes::{FakeDocSearch, FakeRegistry, FakeWriteService};
use ports::{DocError, DocSearch, ReadError, RegistryRead, WriteError, WriteService};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Shared by the spies: the calls made, in order, as `method` plus a description of the arguments that matter.
#[derive(Debug, Clone, Default)]
pub struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    pub fn push(&self, entry: String) {
        lock(&self.0).push(entry);
    }

    pub fn all(&self) -> Vec<String> {
        lock(&self.0).clone()
    }

    pub fn of(&self, method: &str) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|e| e == method || e.starts_with(&format!("{method} ")))
            .collect()
    }

    pub fn clear(&self) {
        lock(&self.0).clear();
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// RegistryRead
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct SpyRegistry {
    pub inner: FakeRegistry,
    pub log: Log,
    failures: Arc<Mutex<HashMap<&'static str, ReadError>>>,
    delay: Arc<Mutex<Option<Duration>>>,
    /// Answers for methods the fake does not compute, by method.
    pub unavailable: Arc<Mutex<Vec<&'static str>>>,
    /// File contents the fake cannot make (a denied file, a binary one, a file without bytes).
    contents: Arc<Mutex<HashMap<(SwimlaneId, NfsPath), FileContent>>>,
}

impl std::fmt::Debug for SpyRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpyRegistry")
    }
}

impl SpyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail(&self, method: &'static str, error: ReadError) {
        lock(&self.failures).insert(method, error);
    }

    pub fn set_content(&self, s: &SwimlaneId, p: &NfsPath, content: FileContent) {
        lock(&self.contents).insert((s.clone(), p.clone()), content);
    }

    pub fn delay_every_call(&self, by: Duration) {
        *lock(&self.delay) = Some(by);
    }

    async fn enter(&self, method: &'static str, detail: String) -> Result<(), ReadError> {
        self.log.push(if detail.is_empty() {
            method.to_owned()
        } else {
            format!("{method} {detail}")
        });
        let delay = *lock(&self.delay);
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        match lock(&self.failures).get(method) {
            Some(error) => Err(*error),
            None => Ok(()),
        }
    }
}

fn page_text(page: &Page) -> String {
    format!(
        "limit={} cursor={}",
        page.limit(),
        page.cursor().map_or("-", |c| c.as_str())
    )
}

#[async_trait]
impl RegistryRead for SpyRegistry {
    async fn swimlanes(&self, u: &User) -> Result<Vec<SwimlaneSummary>, ReadError> {
        self.enter("swimlanes", String::new()).await?;
        self.inner.swimlanes(u).await
    }

    async fn tree(
        &self,
        u: &User,
        s: &SwimlaneId,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.enter("tree", format!("{s} prefix={prefix} {}", page_text(&page)))
            .await?;
        self.inner.tree(u, s, prefix, page).await
    }

    async fn content(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        src: ContentSource,
    ) -> Result<FileContent, ReadError> {
        self.enter("content", format!("{s} {p}")).await?;
        if let Some(content) = lock(&self.contents).get(&(s.clone(), p.clone())) {
            return Ok(content.clone());
        }
        self.inner.content(u, s, p, src).await
    }

    async fn history(
        &self,
        u: &User,
        s: &SwimlaneId,
        p: &NfsPath,
        page: Page,
    ) -> Result<Paged<Version>, ReadError> {
        self.enter("history", format!("{s} {p} {}", page_text(&page)))
            .await?;
        self.inner.history(u, s, p, page).await
    }

    async fn compare(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        p: Option<LogicalFile>,
    ) -> Result<Comparison, ReadError> {
        self.enter("compare", String::new()).await?;
        self.inner.compare(u, l, r, p).await
    }

    async fn compare_tree(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.enter("compare_tree", page_text(&page)).await?;
        self.inner.compare_tree(u, l, r, page).await
    }

    async fn grid(&self, u: &User, q: GridQuery) -> Result<Grid, ReadError> {
        self.enter(
            "grid",
            format!(
                "file={} swimlanes={} channel={:?} only_differing={} {}",
                q.file,
                q.swimlanes.len(),
                q.channel.as_ref().map(ToString::to_string),
                q.only_differing,
                page_text(&q.page)
            ),
        )
        .await?;
        self.inner.grid(u, q).await
    }

    async fn search_settings(&self, u: &User, q: SettingsQuery) -> Result<Paged<SettingHit>, ReadError> {
        self.enter(
            "search_settings",
            format!(
                "q={} swimlanes={} {}",
                q.path_contains,
                q.swimlanes.len(),
                page_text(&q.page)
            ),
        )
        .await?;
        self.inner.search_settings(u, q).await
    }

    async fn search_text(&self, u: &User, q: TextQuery) -> Result<TextResults, ReadError> {
        self.enter(
            "search_text",
            format!(
                "regex={} case={} max_total={} max_per_file={} glob={:?}",
                q.regex,
                q.case_sensitive,
                q.max_total,
                q.max_per_file,
                q.path_glob.as_ref().map(ToString::to_string)
            ),
        )
        .await?;
        self.inner.search_text(u, q).await
    }

    async fn findings(&self, u: &User, q: FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        self.enter(
            "findings",
            format!(
                "swimlane={:?} kinds={} min={:?} {}",
                q.swimlane.as_ref().map(ToString::to_string),
                q.kinds.len(),
                q.min_severity,
                page_text(&q.page)
            ),
        )
        .await?;
        self.inner.findings(u, q).await
    }

    async fn effective(
        &self,
        u: &User,
        s: &SwimlaneId,
        f: &LogicalFile,
    ) -> Result<EffectiveConfig, ReadError> {
        self.enter("effective", String::new()).await?;
        self.inner.effective(u, s, f).await
    }

    async fn pending_restarts(&self, u: &User, s: &SwimlaneId) -> Result<Vec<ServiceState>, ReadError> {
        self.enter("pending_restarts", s.to_string()).await?;
        self.inner.pending_restarts(u, s).await
    }

    async fn effective_for(
        &self,
        u: &User,
        s: &SwimlaneId,
        q: &EffectiveQuery,
    ) -> Result<EffectiveConfig, ReadError> {
        self.enter(
            "effective_for",
            format!(
                "{s} file={} tenant={} channel={:?} prefix={:?}",
                q.file,
                q.tenant,
                q.channel.as_ref().map(ToString::to_string),
                q.setting_prefix.as_ref().map(ToString::to_string)
            ),
        )
        .await?;
        self.inner.effective_for(u, s, q).await
    }

    async fn compare_in(&self, u: &User, q: &CompareQuery) -> Result<Comparison, ReadError> {
        self.enter(
            "compare_in",
            format!(
                "{} {} file={:?} channel={:?}",
                q.left,
                q.right,
                q.file.as_ref().map(ToString::to_string),
                q.channel.as_ref().map(ToString::to_string)
            ),
        )
        .await?;
        self.inner.compare_in(u, q).await
    }

    async fn compare_tree_under(
        &self,
        u: &User,
        l: CompareRef,
        r: CompareRef,
        prefix: &NfsPath,
        page: Page,
    ) -> Result<TreeComparison, ReadError> {
        self.enter(
            "compare_tree_under",
            format!("prefix={prefix} {}", page_text(&page)),
        )
        .await?;
        self.inner.compare_tree_under(u, l, r, prefix, page).await
    }

    async fn drift(
        &self,
        u: &User,
        s: &SwimlaneId,
        q: &DriftQuery,
        page: Page,
    ) -> Result<Paged<FileEntry>, ReadError> {
        self.enter(
            "drift",
            format!(
                "{s} state={:?} prefix={:?} {}",
                q.state,
                q.prefix.as_ref().map(ToString::to_string),
                page_text(&page)
            ),
        )
        .await?;
        self.inner.drift(u, s, q, page).await
    }

    async fn auto_notify(&self, u: &User, s: &SwimlaneId) -> Result<bool, ReadError> {
        self.enter("auto_notify", s.to_string()).await?;
        if lock(&self.unavailable).contains(&"auto_notify") {
            return Err(ReadError::Unavailable);
        }
        self.inner.auto_notify(u, s).await
    }

    async fn served(&self, u: &User, s: &SwimlaneId, r: &ServeRequest) -> Result<ServedFile, ReadError> {
        self.enter(
            "served",
            format!(
                "{s} app={} tenant={} channel={:?} file={}",
                r.application,
                r.tenant,
                r.channel.as_ref().map(ToString::to_string),
                r.file
            ),
        )
        .await?;
        self.inner.served(u, s, r).await
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// DocSearch
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct SpyDocs {
    pub inner: FakeDocSearch,
    pub log: Log,
    failures: Arc<Mutex<HashMap<&'static str, DocError>>>,
}

impl std::fmt::Debug for SpyDocs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpyDocs")
    }
}

impl SpyDocs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail(&self, method: &'static str, error: DocError) {
        lock(&self.failures).insert(method, error);
    }

    fn enter(&self, method: &'static str, detail: String) -> Result<(), DocError> {
        self.log.push(if detail.is_empty() {
            method.to_owned()
        } else {
            format!("{method} {detail}")
        });
        match lock(&self.failures).get(method) {
            Some(error) => Err(*error),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl DocSearch for SpyDocs {
    async fn search(&self, u: &User, q: &str, limit: usize) -> Result<Vec<DocHit>, DocError> {
        self.enter("search", format!("limit={limit}"))?;
        self.inner.search(u, q, limit).await
    }

    async fn related(&self, u: &User, f: &LogicalFile) -> Result<Vec<DocHit>, DocError> {
        self.enter("related", String::new())?;
        self.inner.related(u, f).await
    }

    async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError> {
        self.enter("read", format!("{path}"))?;
        self.inner.read(u, path, lines).await
    }

    async fn grep(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        self.enter("grep", String::new())?;
        self.inner.grep(u, pattern, regex, context).await
    }

    async fn feature_status(&self, u: &User, q: FeatureQuery) -> Result<FeatureStatus, DocError> {
        self.enter(
            "feature_status",
            format!(
                "flag={:?} swimlanes={}",
                q.flag.as_ref().map(ToString::to_string),
                q.swimlanes.len()
            ),
        )?;
        self.inner.feature_status(u, q).await
    }

    async fn related_to(
        &self,
        u: &User,
        f: &LogicalFile,
        setting: Option<&SettingPath>,
        limit: usize,
    ) -> Result<Vec<DocHit>, DocError> {
        self.enter(
            "related_to",
            format!("setting={:?} limit={limit}", setting.map(ToString::to_string)),
        )?;
        self.inner.related_to(u, f, setting, limit).await
    }

    async fn grep_in(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
        prefix: Option<&DocPath>,
        limit: usize,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        self.enter(
            "grep_in",
            format!(
                "regex={regex} context={context} prefix={:?} limit={limit}",
                prefix.map(ToString::to_string)
            ),
        )?;
        self.inner
            .grep_in(u, pattern, regex, context, prefix, limit)
            .await
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// WriteService
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct SpyWrites {
    pub inner: FakeWriteService,
    /// One entry per call: the method, then for the writes the idempotency key, the door and the user (`method key=.. via=.. user=..`).
    pub log: Log,
    delay: Arc<Mutex<Option<Duration>>>,
}

impl std::fmt::Debug for SpyWrites {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpyWrites")
    }
}

/// The methods that change something or ask the world to: the ones that only a confirmed request may reach.
pub const MUTATING: [&str; 8] = [
    "edit",
    "upload",
    "delete",
    "revert",
    "restart",
    "raise_pr",
    "apply_draft",
    "notify",
];

impl SpyWrites {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every write method waits this long before it runs (a slow write service).
    pub fn delay_writes(&self, by: Duration) {
        *lock(&self.delay) = Some(by);
    }

    /// The calls of the mutating methods, in order.
    pub fn mutations(&self) -> Vec<String> {
        self.log
            .all()
            .into_iter()
            .filter(|e| MUTATING.iter().any(|m| e == m || e.starts_with(&format!("{m} "))))
            .collect()
    }

    fn note(&self, method: &str, ctx: &WriteCtx) {
        self.log.push(format!(
            "{method} key={} via={} user={}",
            ctx.idempotency_key,
            ctx.via.as_str(),
            ctx.user.id
        ));
    }

    async fn wait(&self) {
        let delay = *lock(&self.delay);
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
    }
}

#[async_trait]
impl WriteService for SpyWrites {
    async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
        self.note("edit", ctx);
        self.wait().await;
        self.inner.edit(ctx, r).await
    }
    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError> {
        self.note("upload", ctx);
        self.wait().await;
        self.inner.upload(ctx, r).await
    }
    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError> {
        self.note("delete", ctx);
        self.wait().await;
        self.inner.delete(ctx, r).await
    }
    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError> {
        self.note("revert", ctx);
        self.wait().await;
        self.inner.revert(ctx, r).await
    }
    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError> {
        self.note("restart", ctx);
        self.wait().await;
        self.inner.restart(ctx, r).await
    }
    async fn raise_pr(&self, ctx: &WriteCtx, r: PrRequest) -> Result<WriteOutcome, WriteError> {
        self.note("raise_pr", ctx);
        self.wait().await;
        self.inner.raise_pr(ctx, r).await
    }
    async fn propose_draft(&self, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError> {
        self.note("propose_draft", ctx);
        self.inner.propose_draft(ctx, r).await
    }
    async fn apply_draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<WriteOutcome, WriteError> {
        self.note("apply_draft", ctx);
        self.wait().await;
        self.inner.apply_draft(ctx, id).await
    }
    async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<ChangePreview, WriteError> {
        self.note("preview_effect", ctx);
        self.inner.preview_effect(ctx, r).await
    }
    async fn notify(&self, ctx: &WriteCtx, r: NotifyRequest) -> Result<WriteOutcome, WriteError> {
        self.note("notify", ctx);
        self.wait().await;
        self.inner.notify(ctx, r).await
    }
    async fn draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<Draft, WriteError> {
        self.note("draft", ctx);
        self.inner.draft(ctx, id).await
    }
}
