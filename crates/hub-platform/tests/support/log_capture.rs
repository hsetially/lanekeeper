//! Captures every `tracing` event on the current thread, with its fields, so a test can prove what the logs never hold.
//!
//! One global subscriber is installed the first time a test asks, and it routes each event to the sink of the thread it
//! happened on. A per-thread default subscriber (`tracing::subscriber::set_default`) would be simpler, but tracing caches
//! whether a call site is enabled, and tests that run in parallel threads race on that cache: a call site first seen by a
//! thread with no subscriber can stay disabled for the thread that has one, and the capture comes back empty at random.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex, Once, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

type Sink = Arc<Mutex<Vec<String>>>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

static INSTALL: Once = Once::new();

/// The global subscriber: everything is enabled, and an event goes to this thread's sink, if it has one.
struct Router;

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl Subscriber for Router {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        SINK.with(|sink| {
            if let Some(sink) = sink.borrow().as_ref() {
                let mut line = Line(format!(
                    "{} {}",
                    event.metadata().level(),
                    event.metadata().target()
                ));
                event.record(&mut line);
                sink.lock().unwrap_or_else(PoisonError::into_inner).push(line.0);
            }
        });
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// What one test captured.
#[derive(Clone, Default)]
pub struct LogCapture {
    lines: Sink,
}

/// Stops the capture on this thread when dropped.
pub struct CaptureGuard;

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        SINK.with(|sink| *sink.borrow_mut() = None);
    }
}

impl LogCapture {
    /// Capture on this thread until the guard drops.
    pub fn install() -> (Self, CaptureGuard) {
        INSTALL.call_once(|| {
            // Another global subscriber would be a bug in the test binary, and then captures would be empty, which the
            // tests that need events assert against.
            let _ = tracing::subscriber::set_global_default(Router);
        });
        let capture = Self::default();
        SINK.with(|sink| *sink.borrow_mut() = Some(Arc::clone(&capture.lines)));
        (capture, CaptureGuard)
    }

    pub fn text(&self) -> String {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
}
