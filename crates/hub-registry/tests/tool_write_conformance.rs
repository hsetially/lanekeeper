//! The port suites of `ports::ToolWrites`, run against the registry over a real Postgres (`ports::conformance::tool_writes_with_deletes`
//! and `tool_writes_bounded`): a registration exists only if its transaction commits, a repeat is a no-op, the user is not part of the key,
//! a delete registration is a registration of its own kind, a transaction of another store is refused, and the bound is `Full` at
//! `register` and never at the commit.
//!
//! The suites cannot see the savepoint, the TTL, the matching or the late commit: those are `tests/tool_write_register.rs` and
//! `tests/tool_write_matching.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use hub_registry::RegistryConfig;
use support::rig::Rig;
use support::tool_writes::{PgTxFactory, Probe};

fn probe(rig: &Rig) -> Probe {
    Probe {
        registry: rig.tool_writes.clone(),
        pool: rig.pool.clone(),
    }
}

#[tokio::test]
async fn the_registry_holds_the_port_suite_for_writes_and_deletes() {
    let Some(rig) = Rig::new().await else { return };
    // The suite registers in `sit1` and in `sit2`: a registration belongs to a swimlane (a foreign key, and a cascade).
    rig.add_swimlane("sit2", &[], false).await;
    ports::conformance::tool_writes_with_deletes(&probe(&rig), &PgTxFactory(rig.pool.clone())).await;
}

#[tokio::test]
async fn the_registry_holds_the_bound_exactly_over_both_kinds() {
    let config = RegistryConfig {
        tool_write_max_pending: 6,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    ports::conformance::tool_writes_bounded(&probe(&rig), &PgTxFactory(rig.pool.clone()), 6).await;
}

#[tokio::test]
async fn the_smallest_bound_the_suite_runs_with_holds_too() {
    let config = RegistryConfig {
        tool_write_max_pending: 2,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    ports::conformance::tool_writes_bounded(&probe(&rig), &PgTxFactory(rig.pool.clone()), 2).await;
}
