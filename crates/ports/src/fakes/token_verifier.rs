use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::util::lock;
use crate::conformance::TokenFixtures;
use crate::{AuthError, GoogleIdentity, MAX_TOKEN_BYTES, TokenVerifier, VerifiedUser};
use async_trait::async_trait;

enum EntraEntry {
    Valid(VerifiedUser),
    Expired,
}

#[derive(Default)]
struct State {
    entra: HashMap<String, EntraEntry>,
    /// token -> (audience it was issued for, identity)
    google: HashMap<String, (String, GoogleIdentity)>,
}

/// Accepts exactly the tokens it was told about. No cryptography.
#[derive(Clone, Default)]
pub struct FakeTokenVerifier {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for FakeTokenVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Tokens are credentials: never print them.
        f.debug_struct("FakeTokenVerifier").finish_non_exhaustive()
    }
}

impl FakeTokenVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept_entra(&self, token: &str, user: VerifiedUser) {
        lock(&self.state)
            .entra
            .insert(token.to_owned(), EntraEntry::Valid(user));
    }

    pub fn accept_expired_entra(&self, token: &str) {
        lock(&self.state)
            .entra
            .insert(token.to_owned(), EntraEntry::Expired);
    }

    pub fn accept_google(&self, token: &str, audience: &str, identity: GoogleIdentity) {
        lock(&self.state)
            .google
            .insert(token.to_owned(), (audience.to_owned(), identity));
    }

    /// A verifier that accepts exactly the tokens in `f` (see `conformance::sample::token_fixtures`).
    pub fn from_fixtures(f: &TokenFixtures) -> Self {
        let me = Self::new();
        me.accept_entra(&f.entra_ok.0, f.entra_ok.1.clone());
        me.accept_expired_entra(&f.entra_expired);
        me.accept_google(&f.google_ok.0, &f.google_ok.1, f.google_ok.2.clone());
        me
    }
}

#[async_trait]
impl TokenVerifier for FakeTokenVerifier {
    async fn entra_access_token(&self, jwt: &str) -> Result<VerifiedUser, AuthError> {
        if jwt.len() > MAX_TOKEN_BYTES {
            return Err(AuthError::InvalidToken);
        }
        match lock(&self.state).entra.get(jwt) {
            Some(EntraEntry::Valid(u)) => Ok(u.clone()),
            Some(EntraEntry::Expired) => Err(AuthError::Expired),
            None => Err(AuthError::InvalidToken),
        }
    }

    async fn google_id_token(&self, jwt: &str, aud: &str) -> Result<GoogleIdentity, AuthError> {
        if jwt.len() > MAX_TOKEN_BYTES {
            return Err(AuthError::InvalidToken);
        }
        match lock(&self.state).google.get(jwt) {
            Some((expected, id)) if expected == aud => Ok(id.clone()),
            Some(_) => Err(AuthError::WrongAudience),
            None => Err(AuthError::InvalidToken),
        }
    }
}
