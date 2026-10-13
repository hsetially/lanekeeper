//! Capture everything the agent logs, at every level, so a test can prove that nothing secret is in it (S10, S21).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::subscriber::DefaultGuard;
use tracing_subscriber::fmt::MakeWriter;

/// A subscriber that answers "yes" to every callsite and drops everything it is given. It is
/// installed once per process as the global default; see [`LogCapture::install`].
struct KeepAlive;

impl tracing::Subscriber for KeepAlive {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn event(&self, _event: &tracing::Event<'_>) {}

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

static KEEP_ALIVE: OnceLock<()> = OnceLock::new();

#[derive(Clone, Default)]
pub struct LogCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl LogCapture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route this thread's `tracing` events here until the guard is dropped. Use it in a test on the
    /// current-thread runtime, so that every task runs on the thread that installed it.
    ///
    /// `tracing` fixes, per callsite and for the rest of the process, whether that callsite's events
    /// are dispatched at all: the interest is computed once, when the callsite is first hit, against
    /// whatever subscriber the hitting thread has then. In a test binary there is no global
    /// subscriber, so a callsite first hit on a thread without a capture — by a capture-less test
    /// running in parallel, say — is cached as "never" and then stays silent even here. The
    /// once-per-process [`KeepAlive`] global makes every later first hit "always", and the rebuild
    /// re-evaluates everything registered before it. Threads with no capture still drop their events
    /// when they dispatch.
    ///
    /// A test binary that uses [`LogCapture::install_global`] must not also use this, because the
    /// first `install` claims the global default.
    pub fn install(&self) -> DefaultGuard {
        if KEEP_ALIVE.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(KeepAlive);
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(self.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        tracing::callsite::rebuild_interest_cache();
        guard
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
