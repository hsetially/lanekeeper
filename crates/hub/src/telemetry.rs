//! Logs and traces (T1, S10, S21).
//!
//! **Logs** are one JSON object per line on stdout. JSON escapes every control character, so a path or a name with a newline
//! in it cannot forge a second line. The hub's own targets log at `info`; libraries are held at `warn`.
//!
//! **Traces** are exported over OTLP gRPC to an in-cluster collector, which forwards to Cloud Trace (plan A9). The exporter
//! exists only when `LK_OTLP_ENDPOINT` is set. Spans carry ids and route templates, never bodies, headers or query strings.

use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry};

use crate::config::HubSettings;

/// Why telemetry could not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TelemetryError {
    #[error("the OTLP exporter could not be built")]
    Exporter,
    #[error("a global subscriber is already installed")]
    AlreadyInstalled,
}

/// Which events are logged: the hub's targets at `info`, everything else at `warn`.
#[must_use]
pub fn filter() -> Targets {
    Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target("hub", LevelFilter::INFO)
        .with_target("hub_platform", LevelFilter::INFO)
}

/// A subscriber that writes JSON lines to `writer`, with an OTLP layer when `tracer` is given. The process uses [`init`];
/// tests give it a buffer.
pub fn subscriber<W>(
    writer: W,
    tracer: Option<opentelemetry_sdk::trace::SdkTracer>,
) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let output = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        // The request span carries the request id and the method only, so each line can be tied to a request.
        .with_current_span(true)
        .with_span_list(false)
        .with_writer(writer)
        .with_filter(filter());
    let otel_layer = tracer.map(|t| {
        tracing_opentelemetry::layer()
            .with_tracer(t)
            .with_filter(filter())
    });
    Registry::default().with(output).with(otel_layer)
}

/// Keeps the exporter alive; flushes the spans still queued when dropped.
#[derive(Debug)]
pub struct Telemetry {
    provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    /// Whether spans are being exported.
    #[must_use]
    pub fn exporting(&self) -> bool {
        self.provider.is_some()
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            // Best effort: the process is ending.
            let _ = provider.shutdown();
        }
    }
}

/// The tracer provider for an OTLP endpoint. Building it opens no connection; the exporter connects on first use.
pub fn tracer_provider(endpoint: &str) -> Result<SdkTracerProvider, TelemetryError> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| TelemetryError::Exporter)?;
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::builder().with_service_name("lanekeeper-hub").build())
        .build())
}

/// Install the process-wide subscriber.
pub fn init(settings: &HubSettings) -> Result<Telemetry, TelemetryError> {
    let provider = settings
        .otlp_endpoint
        .as_deref()
        .map(tracer_provider)
        .transpose()?;
    let tracer = provider.as_ref().map(|p| p.tracer("lanekeeper-hub"));
    tracing::subscriber::set_global_default(subscriber(std::io::stdout, tracer))
        .map_err(|_| TelemetryError::AlreadyInstalled)?;
    Ok(Telemetry { provider })
}
