//! Captures everything `tracing` writes on the calling thread between `install()` and the drop of its guard, so a test can
//! search it for a secret (S10).
//!
//! One global subscriber is installed once per test binary and routes each event to the buffer of the thread that emitted
//! it. It is not a `tracing::subscriber::set_default` per test, and the reason is a correctness one. `tracing` caches, per
//! callsite, whether any subscriber wants its events. While exactly one thread-local subscriber is alive, a callsite that is
//! first reached on a thread with no subscriber (another test running in parallel, a test that does not capture) is cached as
//! "nobody wants it", and the capturing thread then never sees it. The log tests failed on that: an empty capture when the
//! first hit of `audit chain verification failed` happened in another test. A global subscriber is the one every callsite
//! registers against, whatever thread it first runs on; whether an event is recorded is decided per event, by the calling
//! thread's buffer, and the interest cache says "ask me" (`sometimes`) for every callsite. `tests/log_capture_callsite.rs`
//! drives the failing interleavings.
//!
//! What is captured is every level and span, as with the production formatter, and also what `sqlx` logs about the statements
//! it runs (their text, never their bound values). A test that tampers with a row must therefore bind the text it plants
//! (`AuditRig::admin_sql_bound`) and not write it into the SQL, or the capture finds the test's own statement.
//!
//! Limits: events emitted on another thread (`spawn_blocking`, a multi-thread runtime's workers) are not this test's. Use it
//! with a current-thread runtime, which is what `#[tokio::test]` gives; nothing here changes how that worked before.

use std::cell::RefCell;
use std::io;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing_subscriber::Layer;
use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

type Buffer = Arc<Mutex<Vec<u8>>>;

thread_local! {
    /// The buffer events on this thread go to, or `None` when no test on this thread captures.
    static SINK: RefCell<Option<Buffer>> = const { RefCell::new(None) };
}

fn capturing() -> bool {
    SINK.with(|s| s.borrow().is_some())
}

#[derive(Clone, Default)]
pub struct Capture(Buffer);

impl Capture {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

/// Writes one formatted event into the calling thread's buffer.
pub struct Writer(Option<Buffer>);

impl io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(sink) = &self.0 {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Resolves the buffer at the moment the event is written, on the thread that emitted it.
struct Router;

impl<'a> MakeWriter<'a> for Router {
    type Writer = Writer;

    fn make_writer(&'a self) -> Writer {
        Writer(SINK.with(|s| s.borrow().clone()))
    }
}

static GLOBAL: OnceLock<()> = OnceLock::new();

fn ensure_global_subscriber() {
    GLOBAL.get_or_init(|| {
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(Router)
            .with_ansi(false)
            // Spans and events on a thread that captures nothing are not formatted at all.
            .with_filter(dynamic_filter_fn(|_, _| capturing()));
        let subscriber = tracing_subscriber::registry().with(layer);
        // A second global subscriber in the same binary would take the routing away from the tests: fail loudly.
        assert!(
            tracing::subscriber::set_global_default(subscriber).is_ok(),
            "another global tracing subscriber is installed; the log tests need to own it"
        );
    });
}

/// Ends the capture on this thread when dropped, and restores the capture it replaced (installs nest). It stays on the
/// thread that created it, because the capture is the thread's.
pub struct Guard {
    previous: Option<Buffer>,
    _not_send: PhantomData<*const ()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SINK.with(|s| *s.borrow_mut() = previous);
    }
}

/// Start capturing this thread's events (every level, spans included). Keep the guard alive for the duration of the test.
pub fn install() -> (Capture, Guard) {
    ensure_global_subscriber();
    let capture = Capture::default();
    let previous = SINK.with(|s| s.borrow_mut().replace(Arc::clone(&capture.0)));
    (
        capture,
        Guard {
            previous,
            _not_send: PhantomData,
        },
    )
}
