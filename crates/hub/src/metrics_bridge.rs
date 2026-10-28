//! The identity module's counters on the hub's `/metrics` (plan 18, T4, G14; S9, S10).
//!
//! `hub-identity` keeps its counters as plain atomics behind [`CounterSource`] and has no metrics library, so that the crate that
//! exports them owns the format. This is that line for the hub: a [`Collector`] that, on each scrape, reads the source and writes
//! each counter and gauge under the hub's prefix. The series are the ones the alerts of `hub_identity::metrics` name
//! (`audit_verify_failures_total` and the rest), and they carry no labels, so the exposition is the same size however long the hub
//! runs (S10).
//!
//! The Prometheus text format adds `_total` to a counter's name itself, so the suffix the source uses is taken off here; a name
//! that is registered with it would come out as `..._total_total`.

use std::fmt;
use std::sync::Arc;

use hub_identity::metrics::CounterSource;
use prometheus_client::collector::Collector;
use prometheus_client::encoding::{DescriptorEncoder, NoLabelSet};
use prometheus_client::metrics::MetricType;

/// Reads a [`CounterSource`] when the registry is scraped.
pub struct SourceCollector {
    source: Arc<dyn CounterSource>,
}

impl fmt::Debug for SourceCollector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceCollector").finish_non_exhaustive()
    }
}

impl SourceCollector {
    #[must_use]
    pub fn new(source: Arc<dyn CounterSource>) -> Self {
        Self { source }
    }
}

impl Collector for SourceCollector {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), fmt::Error> {
        for counter in self.source.counters() {
            let name = counter.name.strip_suffix("_total").unwrap_or(counter.name);
            let mut metric = encoder.encode_descriptor(name, counter.help, None, MetricType::Counter)?;
            metric.encode_counter::<NoLabelSet, _, u64>(&counter.value, None)?;
        }
        for gauge in self.source.gauges() {
            let mut metric = encoder.encode_descriptor(gauge.name, gauge.help, None, MetricType::Gauge)?;
            metric.encode_gauge(&gauge.value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hub_identity::metrics::{Counter, Gauge};

    use super::*;
    use crate::metrics::Metrics;

    struct Fixed;

    impl CounterSource for Fixed {
        fn counters(&self) -> Vec<Counter> {
            vec![
                Counter {
                    name: "audit_verify_failures_total",
                    help: "Mismatches found",
                    value: 3,
                },
                Counter {
                    name: "audit_lines_shipped_total",
                    help: "Lines shipped",
                    value: 0,
                },
            ]
        }

        fn gauges(&self) -> Vec<Gauge> {
            vec![Gauge {
                name: "audit_verify_unfinished_runs_in_a_row",
                help: "Runs without a verdict",
                value: 2,
            }]
        }
    }

    #[test]
    fn counters_and_gauges_appear_under_the_hubs_prefix_with_no_labels() {
        let metrics = Metrics::new();
        metrics.register(|registry| {
            registry.register_collector(Box::new(SourceCollector::new(Arc::new(Fixed))));
        });
        let text = metrics.encode();
        assert!(
            text.contains("lanekeeper_audit_verify_failures_total 3\n"),
            "{text}"
        );
        assert!(
            text.contains("lanekeeper_audit_lines_shipped_total 0\n"),
            "{text}"
        );
        assert!(
            text.contains("lanekeeper_audit_verify_unfinished_runs_in_a_row 2\n"),
            "{text}"
        );
        assert!(!text.contains("_total_total"), "{text}");
        assert!(text.contains("# TYPE lanekeeper_audit_verify_unfinished_runs_in_a_row gauge"));
        assert!(text.contains("# TYPE lanekeeper_audit_verify_failures counter"));
        // Nothing the identity module counts has a label: the series count is fixed by the source.
        for line in text.lines().filter(|l| l.contains("audit_")) {
            assert!(!line.contains('{'), "{line}");
        }
    }

    #[test]
    fn the_source_is_read_again_on_every_scrape() {
        use std::sync::atomic::{AtomicU64, Ordering};

        struct Moving(AtomicU64);

        impl CounterSource for Moving {
            fn counters(&self) -> Vec<Counter> {
                vec![Counter {
                    name: "audit_checkpoints_written_total",
                    help: "Checkpoints",
                    value: self.0.load(Ordering::SeqCst),
                }]
            }
        }

        let source = Arc::new(Moving(AtomicU64::new(1)));
        let metrics = Metrics::new();
        metrics.register(|registry| {
            registry.register_collector(Box::new(SourceCollector::new(
                Arc::clone(&source) as Arc<dyn CounterSource>
            )));
        });
        assert!(
            metrics
                .encode()
                .contains("lanekeeper_audit_checkpoints_written_total 1\n")
        );
        source.0.store(5, Ordering::SeqCst);
        assert!(
            metrics
                .encode()
                .contains("lanekeeper_audit_checkpoints_written_total 5\n")
        );
    }
}
