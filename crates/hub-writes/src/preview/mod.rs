//! Previews (D46): what an upload or a delete would do, and the token that lets it through.
//!
//! - [`effect`]: `preview_effect`, from the engine's `effect_preview` over the layers of the logical file.
//! - [`token`]: the HMAC token bound to the user, swimlane, path, action and hashes, valid for ten minutes.

pub(crate) mod effect;
pub(crate) mod token;

use domain::PreviewId;

use crate::error::{Internal, Res};
use crate::service::Inner;
use token::{Binding, verify};

impl Inner {
    /// Whether `presented` is a token issued for exactly this request and still alive. A missing token is a refusal, not an error;
    /// only a key that cannot be had is an error (the request cannot be judged).
    pub(crate) async fn preview_ok(&self, binding: &Binding<'_>, presented: Option<&PreviewId>) -> Res<bool> {
        let Some(id) = presented else {
            return Ok(false);
        };
        let key = self
            .preview_key
            .get(self.cfg.db_timeout)
            .await
            .map_err(|_| Internal::new("preview", "key"))?;
        match verify(&key, binding, id, self.now_ms()) {
            Ok(()) => Ok(true),
            Err(why) => {
                // The reason is one of four fixed words; the token itself is not logged.
                tracing::info!(reason = %why, "a preview id was refused");
                Ok(false)
            }
        }
    }
}
