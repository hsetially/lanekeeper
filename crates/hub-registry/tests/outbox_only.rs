//! D80 (T9): every domain event goes out through the transactional outbox, in the transaction of the change it announces, and nowhere else.
//!
//! Two proofs, one of the source and one of the behaviour. The source: nothing in `src/` calls `EventBus::publish` (the one method that fans an
//! event out with no transaction behind it), in any of the ways a call can be written, and every module that makes an event publishes it
//! with `publish_in_tx` or `publish_many_in_tx` (or the crate's `publish_all`, which is those two). The behaviour: a transaction that
//! rolls back leaves no event in the outbox, whether the bus is used directly or by the code that ingests a scan and the code that
//! recomputes a swimlane, and the same work that then commits leaves its events.
//!
//! `tests/hygiene.rs` keeps its substring check for `.publish(`. This scan is the one that also sees what a substring cannot: a call
//! split over lines, a call by path (`EventBus::publish(&bus, e)`), a call with a turbofish, and a decorator that defines `publish` itself.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use domain::{ContentHash, DomainEvent, FileClass, NfsPath, ObservationSource, SwimlaneId};
use engine::drift_hash;
use ports::{EventBus as _, SinkError, Tx, content_hash};
use support::rig::{Rig, T, delta, entry, root};
use support::world::{World, state_of};

// ---------------------------------------------------------------------------------------------------------------- the source

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `(file name below src/, production text)` of every source file: what comes before the first `#[cfg(test)]` line, without comment
/// lines (a doc comment may show a call in prose).
fn sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    files
        .into_iter()
        .map(|f| {
            let name = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            let text: Vec<String> = fs::read_to_string(&f)
                .unwrap()
                .lines()
                .take_while(|l| l.trim() != "#[cfg(test)]")
                .filter(|l| !l.trim_start().starts_with("//"))
                .map(str::to_owned)
                .collect();
            (name, text.join("\n"))
        })
        .collect()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The turbofish at the start of `rest` (`::<A, B<C>>`) skipped, if there is one.
fn after_turbofish(rest: &str) -> &str {
    let rest = rest.trim_start();
    let Some(inner) = rest.strip_prefix("::") else {
        return rest;
    };
    let inner = inner.trim_start();
    if !inner.starts_with('<') {
        return rest;
    }
    let mut depth = 0_usize;
    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return &inner[i + 1..];
                }
            }
            _ => {}
        }
    }
    rest
}

/// Every place where `publish` itself (not `publish_in_tx`, not `publish_all`) is called or defined: `x.publish(`, `x\n    .publish (`,
/// `EventBus::publish(`, `<dyn EventBus>::publish(`, `x.publish::<T>(`, and `fn publish(` (an implementation of the trait). The text
/// around each, for the message.
fn direct_publishes(production: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = production[from..].find("publish") {
        let at = from + i;
        from = at + "publish".len();
        // A word that has `publish` in it (`republish`, `publish_in_tx`, `publisher`) is another word.
        if production[..at].chars().next_back().is_some_and(is_ident)
            || production[from..].chars().next().is_some_and(is_ident)
        {
            continue;
        }
        let before = production[..at].trim_end();
        let defines = before.ends_with("fn") && !before[..before.len() - 2].ends_with(is_ident);
        let calls = before.ends_with('.') || before.ends_with("::");
        if !(defines || calls) {
            continue;
        }
        if after_turbofish(&production[from..]).trim_start().starts_with('(') {
            let context: String = production[at.saturating_sub(24)..(from + 24).min(production.len())]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            found.push(context);
        }
    }
    found
}

#[test]
fn the_scan_finds_every_way_to_write_a_direct_publish_and_nothing_else() {
    for plant in [
        "bus.publish(event).await",
        "self.bus\n        .publish(event)\n        .await",
        "bus.publish (event)",
        "bus\n    .publish\n    (event)",
        "ports::EventBus::publish(&bus, event)",
        "<dyn EventBus>::publish(&*bus, event)",
        "bus.publish::<DomainEvent>(event)",
        "bus.publish::<Vec<Option<u8>>>(event)",
        "impl EventBus for Wrapper {\n    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> { Ok(()) }\n}",
    ] {
        assert_eq!(direct_publishes(plant).len(), 1, "the scan missed: {plant}");
    }
    for clean in [
        "bus.publish_in_tx(tx, event).await",
        "bus.publish_many_in_tx(tx, events).await",
        "crate::batch::publish_all(&*self.bus, &mut handle, events).await",
        "fn publish_moves(&self) {}",
        "let republish = 1;",
        "let publisher = 2; publisher.run();",
        "let publish = 3; publish + 1",
        "// bus.publish(event) in a comment line",
        "enum Op { Publish }",
    ] {
        assert!(
            direct_publishes(&production_of(clean)).is_empty(),
            "the scan accused: {clean}"
        );
    }
}

/// What the scan would see of a snippet that is a whole file (comment lines out).
fn production_of(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The modules that match on events to route them and never make one: they read `DomainEvent::`, they do not publish it.
const ONLY_READS_EVENTS: [&str; 2] = ["index/git.rs", "pipeline/affected.rs"];

#[test]
fn every_event_goes_through_publish_in_tx() {
    let files = sources();
    assert!(
        files.len() >= 40,
        "the scan must cover the crate: {} files",
        files.len()
    );
    let mut publishers = 0;
    for (name, production) in &files {
        let direct = direct_publishes(production);
        assert!(
            direct.is_empty(),
            "{name}: D80, an event goes through publish_in_tx so that it commits with its change: {direct:?}"
        );
        let in_tx = production.contains("publish_in_tx")
            || production.contains("publish_many_in_tx")
            || production.contains("publish_all(");
        if in_tx {
            publishers += 1;
        }
        if production.contains("DomainEvent::") && !ONLY_READS_EVENTS.contains(&name.as_str()) {
            assert!(
                in_tx,
                "{name} makes a domain event and does not publish it in a transaction (D80); if it only reads events, say so in ONLY_READS_EVENTS"
            );
        }
    }
    assert!(
        publishers >= 6,
        "the scan sees the modules that publish: {publishers}"
    );
}

// ---------------------------------------------------------------------------------------------------------------- the behaviour

fn event(n: u8) -> DomainEvent {
    DomainEvent::FileObserved {
        swimlane: SwimlaneId::parse("sit1").unwrap(),
        path: NfsPath::parse("a.yml").unwrap(),
        hash: content_hash(&[n]),
        source: ObservationSource::Scan,
    }
}

#[tokio::test]
async fn a_rolled_back_transaction_leaves_no_event_and_a_committed_one_leaves_each() {
    let Some(rig) = Rig::new().await else { return };
    assert_eq!(rig.bus.count().await, 0);

    // One event, then a batch, in a transaction that rolls back.
    let mut tx = rig.pool.begin().await.unwrap();
    rig.bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), event(1))
        .await
        .unwrap();
    rig.bus
        .publish_many_in_tx(&mut Tx::from_pg(&mut tx), vec![event(2), event(3)])
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        rig.bus.count().await,
        0,
        "nothing is in the outbox: nothing can be fanned out"
    );

    // The same, committed.
    let mut tx = rig.pool.begin().await.unwrap();
    rig.bus
        .publish_in_tx(&mut Tx::from_pg(&mut tx), event(1))
        .await
        .unwrap();
    rig.bus
        .publish_many_in_tx(&mut Tx::from_pg(&mut tx), vec![event(2), event(3)])
        .await
        .unwrap();
    assert_eq!(
        rig.bus.count().await,
        0,
        "inside the transaction nobody else sees them"
    );
    tx.commit().await.unwrap();
    assert_eq!(
        rig.bus.published().await,
        vec![event(1), event(2), event(3)],
        "in order, once each"
    );
}

#[tokio::test]
async fn a_scan_that_cannot_be_audited_announces_nothing_and_the_retry_announces_it_once() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1"), ("b.yml", "y: 1")]).await;
    let announced = rig.bus.count().await;
    assert_eq!(announced, 2, "the first scan: one FileObserved for each file");

    // The audit event is written after the outbox events, in the same transaction: its failure takes the events back.
    rig.audit.fail_after(Some(0));
    let change = delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("a.yml", b"x: 2", T + 5_000)],
    );
    assert_eq!(rig.delta(change.clone()).await, Err(SinkError::Unavailable));
    assert_eq!(
        rig.bus.count().await,
        announced,
        "the event of a change that was rolled back is not in the outbox"
    );
    assert_eq!(
        rig.state("a.yml").await.unwrap().hash,
        Some(content_hash(b"x: 1"))
    );

    // The audit log is back: the agent sends the same scan again and the change and its event commit together.
    rig.audit.fail_after(None);
    rig.delta(change).await.unwrap();
    let after = rig.bus.published().await;
    assert_eq!(after.len(), announced + 1);
    assert!(
        matches!(after.last(), Some(DomainEvent::FileObserved { hash, .. }) if *hash == content_hash(b"x: 2")),
        "{after:?}"
    );
}

fn drift(text: &str) -> ContentHash {
    drift_hash(Some(&Bytes::from(text.to_owned())), FileClass::Structured).unwrap()
}

#[tokio::test]
async fn a_recompute_that_cannot_be_committed_announces_nothing_and_the_one_that_can_announces_it() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    // An adopted swimlane in sync with Git, then Git moves on: the file is `git_ahead` (announced as drift).
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    w.inventory(&[("app/core.yml", "a: 1\n"), ("app/x-sit1.yml", "x: 1\n")])
        .await;
    w.adopt().await;
    w.baseline("app/core.yml", drift("a: 1\n")).await;
    w.baseline("app/x-sit1.yml", drift("x: 1\n")).await;
    w.whole().await;
    w.base_commit(&[("config/app/core.yml", Some("a: 2\n"))]);
    w.paths(&["app/core.yml"]).await;
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("git_ahead"));
    let announced = w.rig.bus.count().await;

    // NFS gets what Git has: they converge, the swimlane is adopted, so the baseline moves, and that is audited. The audit fails, last, after
    // the rows, the findings and the events were written in the transaction.
    w.nfs_change(&[("app/core.yml", "a: 2\n")]).await;
    let ingested = w.rig.bus.count().await;
    assert_eq!(
        ingested,
        announced + 1,
        "the scan's own FileObserved committed with the scan"
    );
    w.pipeline_audit.fail_after(Some(0));
    let report = w.paths(&["app/core.yml"]).await;
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(
        state_of(&w, "app/core.yml").await.as_deref(),
        Some("git_ahead"),
        "the transaction rolled back: no row moved"
    );
    assert_eq!(
        w.rig.bus.count().await,
        ingested,
        "and none of its events (DriftChanged, FindingsChanged) is in the outbox"
    );

    // The audit log is back: the batch is tried again, from the database, and its events commit with it.
    w.pipeline_audit.fail_after(None);
    let report = w.paths(&["app/core.yml"]).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(state_of(&w, "app/core.yml").await.as_deref(), Some("in_sync"));
    assert!(
        w.rig.bus.count().await > ingested,
        "the events of the recompute that committed are in the outbox"
    );
    let drift_events = w.drift_events().await;
    assert!(
        drift_events
            .last()
            .is_some_and(|paths| paths.contains(&"app/core.yml".to_owned())),
        "{drift_events:?}"
    );
}
