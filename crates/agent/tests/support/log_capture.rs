//! Capture everything the agent logs, at every level, so a test can prove that nothing secret is in it (S10, S21).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::sync::{Arc, Mutex};

use tracing::subscriber::DefaultGuard;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
pub struct LogCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl LogCapture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route this thread's `tracing` events here until the guard is dropped. Use it in a test on the current-thread
    /// runtime, so that every task runs on the thread that installed it.
    pub fn install(&self) -> DefaultGuard {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(self.clone())
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    /// Route the `tracing` events of every thread here, for the rest of the process. For a test whose work happens on
    /// worker threads (the walker's pool, the blocking pool), where [`LogCapture::install`] would see nothing. It can be
    /// called once per test binary, so a binary that uses it holds one test that does.
    pub fn install_global(&self) {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(self.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other test in this binary installed a subscriber");
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
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

impl<'a> MakeWriter<'a> for LogCapture {
    type Writer = Writer;

    fn make_writer(&'a self) -> Writer {
        Writer(Arc::clone(&self.buffer))
    }
}
