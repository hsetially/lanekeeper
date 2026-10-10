//! What operators see of the agent (T7, S16): health probes, Prometheus metrics and JSON logs.
//!
//! - [`health`]: `/healthz`, `/readyz` and `/metrics` on one small port.
//! - [`metrics`]: the counters, gauges and histograms behind `/metrics`; labels come from closed sets.
//! - [`logging`]: JSON lines, a level from `LK_LOG`, and the Kubernetes client crates kept silent (S10).
pub mod health;
pub mod logging;
pub mod metrics;

pub use health::{Health, NotReady, Progress, TaskGuard};
pub use metrics::{Metrics, Operation, Outcome, ScanKind, ScanResult};
