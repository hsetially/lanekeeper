//! The port suite for `RegistryRead` (`ports::conformance::registry_read`), run against the real service over a real Postgres:
//! pending, disabled and role-less users are `Forbidden` on every method (S4), unknown swimlanes are `NotFound`, the tree lists
//! direct children in order and pages by cursor, content is exact, history lists the current version first, and the limits are
//! enforced. The six methods of `contract/07-mcp` (`effective_for`, `compare_in`, `compare_tree_under`, `drift`, `auto_notify`,
//! `served`) are part of the same run: the wrapper `Scenario` forwards every one, so that what is tested is the service and not the
//! port's default `Unavailable`; what the registry does for each beyond the suite is in `port_reads.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use support::reads::{Reads, Scenario};

#[tokio::test]
async fn registry_read_conformance() {
    let Some(reads) = Reads::new(&["sit1"]).await else {
        return;
    };
    ports::conformance::registry_read(&Scenario(reads)).await;
}
