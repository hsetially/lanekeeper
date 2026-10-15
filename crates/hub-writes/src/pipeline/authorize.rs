//! Step 1: who is asking, and may they (S4).

use domain::{Role, User, WriteCtx};
use ports::{AuthError, WriteError};

use crate::service::Inner;

/// The user, as the directory has them now, if they are active and at least `floor`. The `ctx.user` of the request is a snapshot
/// from when the session was made; it names the user and is trusted for nothing else: not the role, not the status and not the
/// `requires_approval` flag.
pub(crate) async fn authorize(hub: &Inner, ctx: &WriteCtx, floor: Role) -> Result<User, WriteError> {
    match hub
        .within("authorize", hub.users.require_active(&ctx.user.id, floor))
        .await
    {
        Ok(Ok(user)) => Ok(user),
        // The directory could not answer: the user is neither allowed nor refused, so the request is not served.
        Ok(Err(AuthError::Unavailable)) | Err(_) => Err(WriteError::Unavailable),
        // Pending, disabled, unknown, or a role below the floor: all the same to the caller (S4).
        Ok(Err(_)) => Err(WriteError::Forbidden),
    }
}
