//! Captures everything `tracing` writes while a closure or future runs, so a test can search it for a secret (S10).

use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

pub struct Writer(Arc<Mutex<Vec<u8>>>);

impl io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Writer;

    fn make_writer(&'a self) -> Writer {
        Writer(Arc::clone(&self.0))
    }
}

/// Install a capturing subscriber for the current thread (every level, spans included). Keep the guard alive for the
/// duration of the test; use it with a current-thread runtime, which is what `#[tokio::test]` gives.
pub fn install() -> (Capture, tracing::subscriber::DefaultGuard) {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    (capture, tracing::subscriber::set_default(subscriber))
}
