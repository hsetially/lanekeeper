//! The Google ID token credential (S5): the node's Workload Identity service account says who it is.
//!
//! `TokenVerifier::google_id_token` checks the signature, issuer, audience (the hub) and expiry. What it cannot know is which
//! swimlane the caller may speak for: that is the service account registered for the swimlane, and the token's `email` must
//! equal it. A swimlane with none registered cannot join this way at all.

use ports::GoogleIdentity;
use sqlx::PgPool;
use subtle::ConstantTimeEq;

/// The service account registered for `swimlane`: `Ok(None)` for a swimlane that does not exist or has none.
pub async fn registered_service_account(
    pool: &PgPool,
    swimlane: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT agent_service_account FROM swimlanes WHERE id = $1")
            .bind(swimlane)
            .fetch_optional(pool)
            .await?;
    Ok(row.flatten())
}

/// Whether the verified identity is the registered service account, compared in constant time and without regard to case
/// (service-account emails are case-insensitive, and an admin typed the registered one). A swimlane without a registered
/// account matches nothing, and takes the same comparison as one that does.
#[must_use]
pub fn email_matches(identity: &GoogleIdentity, registered: Option<&str>) -> bool {
    let presented = identity.email.as_str().to_ascii_lowercase();
    let expected = registered.unwrap_or("").to_ascii_lowercase();
    let equal = presented.as_bytes().ct_eq(expected.as_bytes());
    bool::from(equal) && registered.is_some_and(|r| !r.is_empty())
}
