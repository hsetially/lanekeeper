//! Metrics of the Git mirror, the webhook and the caches (S10).
//!
//! Every label comes from a closed set: the repository (`base` or `tenant`), an outcome, a cache result. A branch, a swimlane, a
//! path, a delivery id or a commit is never a label, so the number of series is fixed.

use std::sync::Arc;
use std::time::Duration;

use domain::RepoKind;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum RepoLabel {
    Base,
    Tenant,
}

impl From<RepoKind> for RepoLabel {
    fn from(kind: RepoKind) -> Self {
        match kind {
            RepoKind::Base => Self::Base,
            RepoKind::Tenant => Self::Tenant,
        }
    }
}

/// How a fetch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum FetchOutcome {
    Ok,
    Failed,
    TimedOut,
    /// Another fetch that started after the request finished first, so none was needed.
    Coalesced,
}

/// What the webhook did with a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum WebhookOutcome {
    /// A push whose fetch was queued.
    Queued,
    /// A push for a branch or tag that was already waiting.
    Coalesced,
    /// The fetch queue was full; the poll will catch the change.
    Dropped,
    /// An event the hub does not act on (and `ping`).
    Ignored,
    /// A delivery seen before (the same signed body).
    Duplicate,
    /// A push for a repository the hub does not mirror, or a deleted ref; a pull request of such a repository, or one with no link.
    NotApplicable,
    BadSignature,
    TooLarge,
    BadRequest,
    /// A `pull_request` whose state was applied.
    PrApplied,
    /// The secret or the database could not be reached.
    Unavailable,
}

/// How a read of a tree index was served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum CacheResult {
    Hit,
    Miss,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct FetchLabels {
    repo: RepoLabel,
    outcome: FetchOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RepoLabels {
    repo: RepoLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct WebhookLabels {
    outcome: WebhookOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct CacheLabels {
    result: CacheResult,
}

fn fetch_histogram() -> Histogram {
    // 10 ms to about 5 minutes.
    Histogram::new(exponential_buckets(0.01, 2.0, 16))
}

#[derive(Debug)]
struct Inner {
    fetches: Family<FetchLabels, Counter>,
    fetch_seconds: Family<RepoLabels, Histogram>,
    readthrough: Family<RepoLabels, Counter>,
    webhooks: Family<WebhookLabels, Counter>,
    announced: Counter,
    queue_dropped: Counter,
    tree_index_cache: Family<CacheLabels, Counter>,
    tree_index_entries: Gauge,
    tree_index_persisted: Counter,
}

/// Cloning shares the series.
#[derive(Debug, Clone)]
pub struct GitMetrics {
    inner: Arc<Inner>,
}

impl Default for GitMetrics {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                fetches: Family::default(),
                fetch_seconds: Family::new_with_constructor(fetch_histogram as fn() -> Histogram),
                readthrough: Family::default(),
                webhooks: Family::default(),
                announced: Counter::default(),
                queue_dropped: Counter::default(),
                tree_index_cache: Family::default(),
                tree_index_entries: Gauge::default(),
                tree_index_persisted: Counter::default(),
            }),
        }
    }
}

impl GitMetrics {
    pub fn register(&self, registry: &mut Registry) {
        let i = &self.inner;
        registry.register(
            "git_fetches",
            "Fetches into the mirrors, by repository and outcome",
            i.fetches.clone(),
        );
        registry.register(
            "git_fetch_seconds",
            "Duration of fetches that ran, by repository",
            i.fetch_seconds.clone(),
        );
        registry.register(
            "git_readthrough_fetches",
            "Fetches started because a commit was missing from the mirror",
            i.readthrough.clone(),
        );
        registry.register(
            "git_webhooks",
            "Webhook deliveries, by outcome",
            i.webhooks.clone(),
        );
        registry.register(
            "git_head_moved_announced",
            "GitHeadMoved events this replica published",
            i.announced.clone(),
        );
        registry.register(
            "git_fetch_queue_dropped",
            "Webhook pushes dropped because the fetch queue was full",
            i.queue_dropped.clone(),
        );
        registry.register(
            "git_tree_index_reads",
            "Tree index reads, by cache result",
            i.tree_index_cache.clone(),
        );
        registry.register(
            "git_tree_index_entries",
            "Entries held by the tree index cache",
            i.tree_index_entries.clone(),
        );
        registry.register(
            "git_tree_index_persisted_rows",
            "Rows written to git_tree_index by this replica",
            i.tree_index_persisted.clone(),
        );
    }

    pub(crate) fn fetch(&self, repo: RepoKind, outcome: FetchOutcome, took: Option<Duration>) {
        let repo = RepoLabel::from(repo);
        self.inner
            .fetches
            .get_or_create(&FetchLabels { repo, outcome })
            .inc();
        if let Some(took) = took {
            self.inner
                .fetch_seconds
                .get_or_create(&RepoLabels { repo })
                .observe(took.as_secs_f64());
        }
    }

    pub(crate) fn readthrough(&self, repo: RepoKind) {
        self.inner
            .readthrough
            .get_or_create(&RepoLabels { repo: repo.into() })
            .inc();
    }

    pub(crate) fn webhook(&self, outcome: WebhookOutcome) {
        self.inner
            .webhooks
            .get_or_create(&WebhookLabels { outcome })
            .inc();
    }

    pub(crate) fn announced(&self) {
        self.inner.announced.inc();
    }

    pub(crate) fn queue_dropped(&self) {
        self.inner.queue_dropped.inc();
    }

    pub(crate) fn tree_index(&self, result: CacheResult) {
        self.inner
            .tree_index_cache
            .get_or_create(&CacheLabels { result })
            .inc();
    }

    pub(crate) fn set_tree_index_entries(&self, n: u64) {
        self.inner
            .tree_index_entries
            .set(i64::try_from(n).unwrap_or(i64::MAX));
    }

    pub(crate) fn tree_index_persisted(&self, rows: u64) {
        self.inner.tree_index_persisted.inc_by(rows);
    }

    #[must_use]
    pub fn fetches_total(&self, repo: RepoKind, outcome: FetchOutcome) -> u64 {
        self.inner
            .fetches
            .get_or_create(&FetchLabels {
                repo: repo.into(),
                outcome,
            })
            .get()
    }

    #[must_use]
    pub fn readthrough_total(&self, repo: RepoKind) -> u64 {
        self.inner
            .readthrough
            .get_or_create(&RepoLabels { repo: repo.into() })
            .get()
    }

    #[must_use]
    pub fn webhooks_total(&self, outcome: WebhookOutcome) -> u64 {
        self.inner
            .webhooks
            .get_or_create(&WebhookLabels { outcome })
            .get()
    }

    #[must_use]
    pub fn announced_total(&self) -> u64 {
        self.inner.announced.get()
    }

    #[must_use]
    pub fn tree_index_reads(&self, result: CacheResult) -> u64 {
        self.inner
            .tree_index_cache
            .get_or_create(&CacheLabels { result })
            .get()
    }

    #[must_use]
    pub fn tree_index_persisted_total(&self) -> u64 {
        self.inner.tree_index_persisted.get()
    }
}
