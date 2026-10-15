//! RED metrics of the tools (S10): calls and errors by tool and outcome, how long a call took, and what the pipeline replaced.
//!
//! Every label comes from a closed set: the 27 tool names and the outcome (`ok` or one of the contract's error codes). A user, a
//! path, a swimlane, a pattern or an id never becomes a label, so the number of series is fixed however long the hub runs.
//! `register` adds them to the hub's registry; cloning shares the series.

use std::sync::Arc;
use std::time::Duration;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct CallLabels {
    tool: &'static str,
    outcome: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ToolLabel {
    tool: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ClassLabel {
    class: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct OutcomeLabel {
    outcome: &'static str,
}

#[derive(Debug)]
struct Inner {
    calls: Family<CallLabels, Counter>,
    duration: Family<ToolLabel, Histogram>,
    rate_limited: Family<ClassLabel, Counter>,
    redactions: Counter,
    redactions_late: Counter,
    cache_lookups: Family<ClassLabel, Counter>,
    confirmations: Family<OutcomeLabel, Counter>,
    tools_list_bytes: Gauge,
}

/// Cloning shares the series.
#[derive(Debug, Clone)]
pub struct McpMetrics {
    inner: Arc<Inner>,
}

fn latency_histogram() -> Histogram {
    // 1 ms to about 33 s, in powers of two.
    Histogram::new(exponential_buckets(0.001, 2.0, 16))
}

impl Default for McpMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl McpMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                calls: Family::default(),
                duration: Family::new_with_constructor(latency_histogram as fn() -> Histogram),
                rate_limited: Family::default(),
                redactions: Counter::default(),
                redactions_late: Counter::default(),
                cache_lookups: Family::default(),
                confirmations: Family::default(),
                tools_list_bytes: Gauge::default(),
            }),
        }
    }

    pub fn register(&self, registry: &mut Registry) {
        let i = &self.inner;
        registry.register(
            "mcp_tool_calls",
            "MCP tool calls, by tool and outcome (`ok` or an error code)",
            i.calls.clone(),
        );
        registry.register(
            "mcp_tool_call_duration_seconds",
            "Time to answer an MCP tool call, by tool",
            i.duration.clone(),
        );
        registry.register(
            "mcp_rate_limited",
            "MCP calls refused for being over a limit, by class (read or write)",
            i.rate_limited.clone(),
        );
        registry.register(
            "mcp_redacted_values",
            "Values the secret scan replaced in MCP results",
            i.redactions.clone(),
        );
        registry.register(
            "mcp_redaction_late",
            "Results in which the final sweep replaced a value after the tool had built them: each one is a defect, a field the typed path missed",
            i.redactions_late.clone(),
        );
        registry.register(
            "mcp_scan_cache_lookups",
            "Scan and settings-index cache lookups, by result",
            i.cache_lookups.clone(),
        );
        registry.register(
            "mcp_confirmations",
            "Confirmations of MCP writes, by how each went (asked, accepted, declined, cancelled, timed_out, aborted, terms_changed, draft_link, replayed)",
            i.confirmations.clone(),
        );
        registry.register(
            "mcp_tools_list_bytes",
            "Size of the tools/list answer of the last caller",
            i.tools_list_bytes.clone(),
        );
    }

    pub(crate) fn call(&self, tool: &'static str, outcome: &'static str, elapsed: Duration) {
        self.inner
            .calls
            .get_or_create(&CallLabels { tool, outcome })
            .inc();
        self.inner
            .duration
            .get_or_create(&ToolLabel { tool })
            .observe(elapsed.as_secs_f64());
    }

    pub(crate) fn rate_limited(&self, class: &'static str) {
        self.inner.rate_limited.get_or_create(&ClassLabel { class }).inc();
    }

    pub(crate) fn redacted(&self, values: u32) {
        self.inner.redactions.inc_by(u64::from(values));
    }

    /// One result needed the final sweep.
    pub(crate) fn redaction_late(&self) {
        self.inner.redactions_late.inc();
    }

    pub(crate) fn cache(&self, result: &'static str) {
        self.inner
            .cache_lookups
            .get_or_create(&ClassLabel { class: result })
            .inc();
    }

    pub(crate) fn confirmation(&self, outcome: &'static str) {
        self.inner
            .confirmations
            .get_or_create(&OutcomeLabel { outcome })
            .inc();
    }

    pub(crate) fn set_tools_list_bytes(&self, bytes: usize) {
        self.inner
            .tools_list_bytes
            .set(i64::try_from(bytes).unwrap_or(i64::MAX));
    }

    /// Calls of `tool` with `outcome` since the start (for tests).
    #[must_use]
    pub fn calls_of(&self, tool: &'static str, outcome: &'static str) -> u64 {
        self.inner
            .calls
            .get_or_create(&CallLabels { tool, outcome })
            .get()
    }

    /// Confirmations that ended `outcome` since the start (for tests).
    #[must_use]
    pub fn confirmations_of(&self, outcome: &'static str) -> u64 {
        self.inner
            .confirmations
            .get_or_create(&OutcomeLabel { outcome })
            .get()
    }

    /// Values replaced since the start (for tests).
    #[must_use]
    pub fn redacted_values(&self) -> u64 {
        self.inner.redactions.get()
    }

    /// Results in which the final sweep replaced a value, since the start; zero while every field takes the typed path (for tests).
    #[must_use]
    pub fn late_redactions(&self) -> u64 {
        self.inner.redactions_late.get()
    }

    /// Calls refused for being over a limit, since the start (for tests).
    #[must_use]
    pub fn rate_limited_calls(&self, class: &'static str) -> u64 {
        self.inner.rate_limited.get_or_create(&ClassLabel { class }).get()
    }
}
