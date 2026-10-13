//! Prometheus metrics (T1, S10): RED per route template.
//!
//! Labels come from closed sets: the route template the router matched (or `unmatched`), the status class, and nothing
//! else. A user, a path parameter, a swimlane or an id never becomes a label (S10), so the number of series is fixed by the
//! route table and the exposition has the same size however long the hub runs. Other modules register their own metrics
//! through [`Metrics::register`] while the hub is built.

use std::sync::{Mutex, PoisonError};

use prometheus_client::encoding::{EncodeLabelSet, text};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RequestLabels {
    route: String,
    class: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct RouteLabel {
    route: String,
}

/// The status class of a response, as a label value from a closed set.
#[must_use]
pub fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

/// The HTTP metrics and the registry other modules add theirs to.
#[derive(Debug)]
pub struct Metrics {
    registry: Mutex<Registry>,
    requests: Family<RequestLabels, Counter>,
    duration: Family<RouteLabel, Histogram>,
    in_flight: Gauge,
}

fn latency_histogram() -> Histogram {
    // 1 ms to about 33 s, in powers of two.
    Histogram::new(exponential_buckets(0.001, 2.0, 16))
}

impl Metrics {
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix("lanekeeper");
        let requests = Family::<RequestLabels, Counter>::default();
        let duration =
            Family::<RouteLabel, Histogram>::new_with_constructor(latency_histogram as fn() -> Histogram);
        let in_flight = Gauge::default();
        registry.register(
            "http_requests",
            "Requests by route template and status class",
            requests.clone(),
        );
        registry.register(
            "http_request_duration_seconds",
            "Time to the response head, by route template",
            duration.clone(),
        );
        registry.register(
            "http_requests_in_flight",
            "Requests being handled now",
            in_flight.clone(),
        );
        Self {
            registry: Mutex::new(registry),
            requests,
            duration,
            in_flight,
        }
    }

    /// Add a module's metrics to the exposition. Call it while the hub is built.
    pub fn register(&self, add: impl FnOnce(&mut Registry)) {
        add(&mut self.registry.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// One request finished. `route` is a template, never a raw path.
    pub fn observe(&self, route: &str, status: u16, seconds: f64) {
        self.requests
            .get_or_create(&RequestLabels {
                route: route.to_owned(),
                class: status_class(status),
            })
            .inc();
        self.duration
            .get_or_create(&RouteLabel {
                route: route.to_owned(),
            })
            .observe(seconds);
    }

    /// A request started; the guard ends it when dropped, including when the request future is cancelled.
    pub fn start(&self) -> InFlight {
        self.in_flight.inc();
        InFlight {
            gauge: self.in_flight.clone(),
        }
    }

    /// The exposition in the `OpenMetrics` text format.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        if text::encode(&mut out, &registry).is_err() {
            out.clear();
        }
        out
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A request in flight.
#[derive(Debug)]
#[must_use = "dropping the guard at once ends the request in the gauge"]
pub struct InFlight {
    gauge: Gauge,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.gauge.dec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_the_route_template_and_status_class_only() {
        let metrics = Metrics::new();
        metrics.observe("/api/v1/blobs/{hash}", 200, 0.002);
        metrics.observe("/api/v1/blobs/{hash}", 404, 0.001);
        let text = metrics.encode();
        assert!(text.contains(r#"route="/api/v1/blobs/{hash}""#), "{text}");
        assert!(text.contains(r#"class="2xx""#));
        assert!(text.contains(r#"class="4xx""#));
        assert!(text.ends_with("# EOF\n"));
    }

    #[test]
    fn in_flight_follows_the_guard() {
        let metrics = Metrics::new();
        let guard = metrics.start();
        assert!(metrics.encode().contains("lanekeeper_http_requests_in_flight 1"));
        drop(guard);
        assert!(metrics.encode().contains("lanekeeper_http_requests_in_flight 0"));
    }

    #[test]
    fn status_classes_are_closed() {
        assert_eq!(status_class(204), "2xx");
        assert_eq!(status_class(429), "4xx");
        assert_eq!(status_class(503), "5xx");
        assert_eq!(status_class(0), "other");
    }
}
