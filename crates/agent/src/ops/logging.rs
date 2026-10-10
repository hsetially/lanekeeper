//! Logging (T7, S10, S21): one JSON object per line on stdout, at the level `LK_LOG` names.
//!
//! JSON escapes every control character, so a path or a name with a newline in it cannot forge a second log line.
//!
//! The level applies to the agent's own targets. Libraries are held at `warn`, and the Kubernetes client crates are
//! off: `kube_runtime` logs the API server's `Status` message at `warn` and `kube_client` logs at `debug`, and those
//! messages can name objects, namespaces and Secrets that this agent has no business writing to a log. The agent logs
//! the HTTP status of a failed call itself, where it knows what it asked for.

use std::io;

use tracing::Level;
use tracing::level_filters::LevelFilter;
use tracing::subscriber::SetGlobalDefaultError;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry};

use crate::config::LogLevel;

/// Targets whose messages can carry text the cluster chose. Never logged, at any level.
pub const SILENCED_TARGETS: [&str; 5] = ["kube", "kube_client", "kube_runtime", "kube_core", "k8s_openapi"];

/// The least severe level the libraries (everything that is not the agent) may log.
const LIBRARY_LEVEL: LevelFilter = LevelFilter::WARN;

fn level_filter(level: LogLevel) -> LevelFilter {
    LevelFilter::from_level(match level {
        LogLevel::Error => Level::ERROR,
        LogLevel::Warn => Level::WARN,
        LogLevel::Info => Level::INFO,
        LogLevel::Debug => Level::DEBUG,
        LogLevel::Trace => Level::TRACE,
    })
}

/// Which events are logged at `level`.
pub fn filter(level: LogLevel) -> Targets {
    let mut targets = Targets::new()
        .with_default(LIBRARY_LEVEL)
        .with_target("agent", level_filter(level));
    for silenced in SILENCED_TARGETS {
        targets = targets.with_target(silenced, LevelFilter::OFF);
    }
    targets
}

/// A subscriber that writes JSON lines to `writer`. The process uses [`init`]; tests give it a buffer.
pub fn subscriber<W>(level: LogLevel, writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let output = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer)
        .with_filter(filter(level));
    Registry::default().with(output)
}

/// Install the process-wide subscriber. Fails if one is already set.
pub fn init(level: LogLevel) -> Result<(), SetGlobalDefaultError> {
    tracing::subscriber::set_global_default(subscriber(level, io::stdout))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::Level;

    use super::*;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Ok(mut inner) = self.0.lock() {
                inner.extend_from_slice(buf);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Buffer;

        fn make_writer(&'a self) -> Buffer {
            self.clone()
        }
    }

    impl Buffer {
        fn text(&self) -> String {
            self.0
                .lock()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        }
    }

    #[test]
    fn the_kubernetes_client_crates_are_silent_at_every_level() {
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let targets = filter(level);
            for silenced in SILENCED_TARGETS {
                for event in [Level::ERROR, Level::WARN, Level::INFO, Level::DEBUG, Level::TRACE] {
                    assert!(
                        !targets.would_enable(silenced, &event),
                        "{silenced} logs at {event} when LK_LOG is {level:?}"
                    );
                    // And the submodules, which is where the messages really come from.
                    assert!(!targets.would_enable(&format!("{silenced}::watcher"), &event));
                }
            }
        }
    }

    #[test]
    fn the_agent_logs_at_the_chosen_level_and_libraries_at_warn() {
        let info = filter(LogLevel::Info);
        assert!(info.would_enable("agent::scan", &Level::INFO));
        assert!(!info.would_enable("agent::scan", &Level::DEBUG));
        let debug = filter(LogLevel::Debug);
        assert!(debug.would_enable("agent::scan", &Level::DEBUG));
        assert!(!debug.would_enable("agent::scan", &Level::TRACE));
        // Libraries never go below warn, whatever the agent's level: h2 and rustls at trace would log framing detail.
        for level in [LogLevel::Info, LogLevel::Trace] {
            let targets = filter(level);
            assert!(targets.would_enable("hyper", &Level::WARN));
            assert!(!targets.would_enable("hyper", &Level::INFO));
            assert!(!targets.would_enable("h2::proto", &Level::DEBUG));
            assert!(!targets.would_enable("rustls", &Level::DEBUG));
        }
    }

    #[test]
    fn a_line_is_one_json_object_whatever_the_message_holds() {
        let buffer = Buffer::default();
        let subscriber = subscriber(LogLevel::Info, buffer.clone());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                path = "a/b.yml",
                note = "line one\nline two \"quoted\"\u{1b}[31m",
                "scan done"
            );
        });
        let text = buffer.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "{text}");
        let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap_or_default();
        assert_eq!(value["fields"]["message"], "scan done");
        assert_eq!(value["fields"]["path"], "a/b.yml");
        assert_eq!(value["level"], "INFO");
        assert!(value["fields"]["note"].as_str().is_some_and(|n| n.contains('\n')));
    }
}
