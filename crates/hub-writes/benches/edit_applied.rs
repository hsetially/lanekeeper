//! P8: an edit is applied on NFS and acknowledged within 1.5 seconds (`P8`, `hub-writes/edit_applied`).
//!
//! What is measured is the whole `WriteService::edit` call: authorize, claim, lock, the engine's validation and secret scan on
//! the blocking pool, the diff, the command to the agent, the blob, and the transaction that records the audit event, the
//! tool-write registration, the outbox event and the response. It runs over a real Postgres (a container, or the server in
//! `LK_TEST_PG_ADMIN_URL`) with the audit chain, the outbox, the blobs and the observations of a 2,000-file swimlane already in it,
//! and a fake agent that answers like the real one (it compares the expected hash and writes).
//!
//! 100 files of 1 KB to 150 KB, including one of 6,000 lines, are edited round-robin; each edit changes one line, as a person
//! does. Every sample is one call (criterion's flat sampling with a measurement time too short for more), so the registered p95 is
//! over single edits. Loopback to a local database and an in-process agent: this bounds the hub's code and the database's time, not
//! the network's; the extra hop of a second replica is covered by `edit_through_a_second_replica_meets_p8`, and prompt 16
//! measures staging.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use domain::{Expected, Role, SwimlaneId};
use ports::WriteService;
use ports::conformance::sample::nfs;
use support::pg::TestDb;
use support::rig::{Rig, ctx, edit};

const FILES: usize = 100;
const OTHER_FILES: usize = 2_000;

/// A YAML document of about `bytes` bytes: many short settings, no secrets, valid.
fn document(seed: usize, bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 64);
    let mut i = 0;
    while s.len() < bytes {
        let _ = writeln!(
            s,
            "service-{seed}.group-{}.setting-{i}: value-{} of the setting",
            i % 37,
            i * 7919 % 1009
        );
        i += 1;
    }
    s
}

fn sizes() -> Vec<usize> {
    // 1 KB up to 150 KB; the file at index 0 is the 6,000-line one.
    (0..FILES).map(|i| 1_000 + (149_000 * i) / (FILES - 1)).collect()
}

#[allow(clippy::too_many_lines)]
fn bench(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let Some(db) = rt.block_on(TestDb::ready()) else {
        panic!("the P8 benchmark needs a database: start Docker or set LK_TEST_PG_ADMIN_URL");
    };
    let rig = rt.block_on(Rig::with_live_registry(db, 8));
    let s: SwimlaneId = rt.block_on(rig.swimlane("sit1", true));
    let editor = rt.block_on(rig.user(1, Role::Editor));

    // The swimlane's files, and a 2,000-file swimlane's observations and a chain with history around them.
    let mut current: Vec<String> = Vec::new();
    for (i, size) in sizes().into_iter().enumerate() {
        let text = if i == 0 {
            // 6,000 lines.
            (0..6_000).fold(String::new(), |mut acc, n| {
                let _ = writeln!(acc, "top.section-{}.key-{n}: value number {n}", n % 40);
                acc
            })
        } else {
            document(i, size)
        };
        rig.agent
            .put(&s, &nfs(&format!("svc-{i}/application.yml")), text.as_bytes());
        current.push(text);
    }
    rt.block_on(async {
        sqlx::query(
            "INSERT INTO file_observations (swimlane_id, path, hash, source, observed_at) \
             SELECT 'sit1', 'other/' || g || '/application.yml', sha256(g::text::bytea), 'scan', now() \
             FROM generate_series(1, $1) g",
        )
        .bind(i32::try_from(OTHER_FILES).unwrap())
        .execute(&rig.pool)
        .await
        .unwrap();
        // A chain with history, as in service.
        for n in 0..300 {
            let mut e = ports::AuditEvent::new(
                domain::Timestamp::from_unix_millis(support::rig::START + n),
                ports::AuditActor::System {
                    component: domain::ShortText::parse("bench").unwrap(),
                },
                ports::AuditAction::SettingsChanged,
                ports::AuditVia::System,
            );
            e.swimlane = Some(s.clone());
            let mut tx = rig.pool.begin().await.unwrap();
            ports::AuditLog::record(&*rig.audit, &mut ports::Tx::from_pg(&mut tx), e)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });

    let mut group = c.benchmark_group("hub-writes");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(200)
        .warm_up_time(Duration::from_secs(1))
        // Too short for more than one iteration per sample, which is what makes each sample one edit.
        .measurement_time(Duration::from_millis(100));
    let mut n = 0_usize;
    group.bench_function("edit_applied", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    n += 1;
                    let file = n % FILES;
                    let old = &current[file];
                    // One line changes, as in a person's edit.
                    let target = (n * 31) % old.lines().count();
                    let new: String = old
                        .lines()
                        .enumerate()
                        .map(|(i, l)| {
                            if i == target {
                                format!("{l} edited {n}\n")
                            } else {
                                format!("{l}\n")
                            }
                        })
                        .collect();
                    let req = edit(
                        &s,
                        &format!("svc-{file}/application.yml"),
                        Expected::Hash {
                            hash: ports::content_hash(old.as_bytes()),
                        },
                        &new,
                    );
                    let started = Instant::now();
                    let out = rig
                        .hub
                        .edit(&ctx(&editor, &format!("bench-{n}")), req)
                        .await
                        .unwrap();
                    total += started.elapsed();
                    assert!(matches!(out, domain::WriteOutcome::Applied { .. }), "{out:?}");
                    current[file] = new;
                }
                total.max(Duration::from_nanos(1))
            })
        });
    });
    group.finish();
    // Inside the runtime: dropping the container outside one panics in `testcontainers`.
    let Rig { db, pool, hub, .. } = rig;
    drop(hub);
    rt.block_on(pool.close());
    rt.block_on(db.shutdown());
}

criterion_group!(benches, bench);
criterion_main!(benches);
