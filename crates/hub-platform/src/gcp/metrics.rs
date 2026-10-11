//! Metrics of the Google API calls (T2, S10): count, outcome and latency per operation.
//!
//! Both labels come from closed sets, so the number of series is fixed. No name, key, secret, project or status text
//! becomes a label.

use std::time::Duration;

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

/// Which Google call it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum Operation {
    /// The access token from the metadata server.
    Token,
    /// `secretmanager.versions.access`.
    SecretAccess,
    /// `cryptoKeyVersions.asymmetricSign`.
    KmsSign,
    /// `cryptoKeys.encrypt`.
    KmsEncrypt,
    /// `cryptoKeys.decrypt`.
    KmsDecrypt,
    /// `cryptoKeyVersions.getPublicKey`.
    KmsPublicKey,
}

/// How it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum Outcome {
    Ok,
    /// Google answered 404.
    NotFound,
    /// Google answered another 4xx: the request was refused, retrying will not help.
    Refused,
    /// Retries ran out on a transport failure or a 429 or 5xx.
    Unavailable,
    /// The deadline ran out.
    Timeout,
    /// The answer could not be trusted: malformed, oversized or failing its checksum.
    BadResponse,
    /// The request never left: the host is not on the allowlist, or the request was malformed.
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct CallLabels {
    operation: Operation,
    outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct OperationLabel {
    operation: Operation,
}

/// The counters and the latency histogram. Cloning shares them.
#[derive(Debug, Clone)]
pub struct GoogleMetrics {
    calls: Family<CallLabels, Counter>,
    retries: Family<OperationLabel, Counter>,
    duration: Family<OperationLabel, Histogram>,
}

fn latency_histogram() -> Histogram {
    // 1 ms to about 33 s, in powers of two.
    Histogram::new(exponential_buckets(0.001, 2.0, 16))
}

impl Default for GoogleMetrics {
    fn default() -> Self {
        Self {
            calls: Family::default(),
            retries: Family::default(),
            duration: Family::new_with_constructor(latency_histogram as fn() -> Histogram),
        }
    }
}

impl GoogleMetrics {
    /// Add the metrics to the hub's exposition.
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "google_api_calls",
            "Google API calls by operation and outcome",
            self.calls.clone(),
        );
        registry.register(
            "google_api_retries",
            "Retries of Google API calls by operation",
            self.retries.clone(),
        );
        registry.register(
            "google_api_call_duration_seconds",
            "Time from the first try to the answer, by operation",
            self.duration.clone(),
        );
    }

    /// One call finished, after `elapsed`.
    pub fn record(&self, operation: Operation, outcome: Outcome, elapsed: Duration) {
        self.calls.get_or_create(&CallLabels { operation, outcome }).inc();
        self.duration
            .get_or_create(&OperationLabel { operation })
            .observe(elapsed.as_secs_f64());
    }

    /// A try failed and another follows.
    pub fn retry(&self, operation: Operation) {
        self.retries.get_or_create(&OperationLabel { operation }).inc();
    }
}

#[cfg(test)]
mod tests {
    use prometheus_client::encoding::text;

    use super::*;

    #[test]
    fn series_are_labelled_by_operation_and_outcome_only() {
        let metrics = GoogleMetrics::default();
        let mut registry = Registry::with_prefix("lanekeeper");
        metrics.register(&mut registry);
        metrics.record(Operation::KmsSign, Outcome::Ok, Duration::from_millis(12));
        metrics.record(
            Operation::SecretAccess,
            Outcome::NotFound,
            Duration::from_millis(3),
        );
        metrics.retry(Operation::KmsSign);
        let mut out = String::new();
        assert!(text::encode(&mut out, &registry).is_ok());
        assert!(out.contains(r#"operation="KmsSign",outcome="Ok""#), "{out}");
        assert!(
            out.contains(r#"operation="SecretAccess",outcome="NotFound""#),
            "{out}"
        );
        assert!(
            out.contains("lanekeeper_google_api_retries_total{operation=\"KmsSign\"} 1"),
            "{out}"
        );
        assert!(
            out.contains("lanekeeper_google_api_call_duration_seconds_count"),
            "{out}"
        );
    }
}
