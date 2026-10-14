//! The port suite for `RegistryRead` (`ports::conformance::registry_read`), run against the real service over a real Postgres:
//! pending, disabled and role-less users are `Forbidden` on every method (S4), unknown swimlanes are `NotFound`, the tree lists
//! direct children in order and pages by cursor, content is exact, history lists the current version first, and the limits are
//! enforced.
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
