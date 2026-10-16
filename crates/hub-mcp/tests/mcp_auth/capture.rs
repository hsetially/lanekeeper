//! The log capture of this test binary: what the crate logged on the calling test's thread, from one subscriber for the whole process.
//!
//! One subscriber is installed, once, with `set_global_default`. A scoped `tracing::subscriber::set_default` loses events when other
//! tests run at the same time: tracing caches, per callsite, whether any subscriber wants its events, and with several threads each
//! setting a subscriber of their own the cache can be built from another thread's and the event is dropped (the rmcp spike and the
//! protocol tests say the same; measured here, `logging::` failed 7 of 20 runs of the filter `the_` while it used `set_default`).
//! So no test in this binary calls `set_default`: they call [`start`] and read [`PerThread::text`] at the end. Every test body runs
//! on its own thread (`#[tokio::test]` is a current-thread runtime, and so are the tasks it spawns), and the capture is keyed by the
//! name of that thread, so tests do not see each other's lines. A test that logs from another thread (`spawn_blocking`, a
//! multi-thread runtime) would not see those lines, and must not rely on them.
//!
//! The filter is the hub's own, widened: everything `hub_mcp` and `hub_platform` log at every level, and the rest at `warn`.
//! Call [`start`] *before* building the server if the lines of the build matter (rmcp says one thing about the public origin when
//! it is built); a test that only wants its calls may call it after.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
pub struct PerThread(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

pub struct ThreadWriter(PerThread);

fn thread_name() -> String {
    std::thread::current().name().unwrap_or("unnamed").to_owned()
}

impl io::Write for ThreadWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .0
            .lock()
            .unwrap()
            .entry(thread_name())
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for PerThread {
    type Writer = ThreadWriter;
    fn make_writer(&'a self) -> ThreadWriter {
        ThreadWriter(self.clone())
    }
}

static LOGS: OnceLock<PerThread> = OnceLock::new();

/// Install the capture if no test has yet, and start the calling test's lines empty.
pub fn start() -> &'static PerThread {
    let logs = LOGS.get_or_init(|| {
        let logs = PerThread::default();
        let filter = Targets::new()
            .with_default(LevelFilter::WARN)
            .with_target("hub_mcp", LevelFilter::TRACE)
            .with_target("hub_platform", LevelFilter::TRACE);
        let subscriber = tracing_subscriber::Registry::default().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(logs.clone())
                .with_filter(filter),
        );
        // The first test that gets here installs it. If something else installed one first, the capture would stay empty, and
        // the assertions on what was logged would say so.
        let _ = tracing::subscriber::set_global_default(subscriber);
        logs
    });
    logs.0.lock().unwrap().remove(&thread_name());
    logs
}

impl PerThread {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(
            self.0
                .lock()
                .unwrap()
                .get(&thread_name())
                .map_or(&[][..], Vec::as_slice),
        )
        .into_owned()
    }
}
