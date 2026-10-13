//! What a user may see over SSE (A10, D35).
//!
//! Roles are global (D35): there is no per-swimlane scope to apply, so the default rule is by event kind. The trait lets the
//! registry or the write paths tighten it later without touching the endpoint. The user's own `swimlane` and `kinds` query
//! narrows the stream further; it never widens it.

use domain::{DomainEvent, Role, User};

pub trait EventVisibility: Send + Sync + 'static {
    /// Whether `user` may be told about `event`. [`DomainEvent::Resync`] is not asked about: it says only that the subscriber
    /// lagged, and it always passes.
    fn visible(&self, user: &User, event: &DomainEvent) -> bool;
}

/// The default: an active Viewer or above sees the events of every swimlane; the request for access of a new user is for
/// Admins only. Events carry ids and hashes, never content, so nothing else needs hiding. Internal events
/// ([`DomainEvent::is_internal`]) are for nobody.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoleVisibility;

impl EventVisibility for RoleVisibility {
    fn visible(&self, user: &User, event: &DomainEvent) -> bool {
        match event {
            // Not a rule about who may see it but a refusal for everyone: the SSE edge has already dropped these, and a
            // rule that fell through to the Viewer arm would be the hole behind it.
            e if e.is_internal() => false,
            DomainEvent::Resync => true,
            DomainEvent::AccessRequestCreated { .. } => user.allows(Role::Admin),
            _ => user.allows(Role::Viewer),
        }
    }
}
