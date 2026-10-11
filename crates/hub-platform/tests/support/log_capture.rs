//! Captures every `tracing` event on the current thread, with its fields, so a test can prove what the logs never hold.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::DefaultGuard;
use tracing::{Event, Metadata, Subscriber};

#[derive(Clone, Default)]
pub struct LogCapture {
    lines: Arc<Mutex<Vec<String>>>,
}

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl Subscriber for LogCapture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line(format!(
            "{} {}",
            event.metadata().level(),
            event.metadata().target()
        ));
        event.record(&mut line);
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.0);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

impl LogCapture {
    /// Capture on this thread until the guard drops.
    pub fn install() -> (Self, DefaultGuard) {
        let capture = Self::default();
        let guard = tracing::subscriber::set_default(capture.clone());
        (capture, guard)
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
