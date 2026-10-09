use async_trait::async_trait;
use domain::{Role, User, UserStatus};

use super::sample::{active, user, user_id};
use crate::{AuthError, GoogleIdentity, MAX_TOKEN_BYTES, TokenVerifier, Users, VerifiedUser};

/// Tokens that the verifier under test accepts or rejects in known ways.
#[derive(Debug, Clone)]
pub struct TokenFixtures {
    /// A valid Entra access token and the user it names.
    pub entra_ok: (String, VerifiedUser),
    /// An Entra token that was valid but has expired.
    pub entra_expired: String,
    /// A valid Google ID token, the audience it was issued for, and the identity it names.
    pub google_ok: (String, String, GoogleIdentity),
}

/// - Valid tokens return their identity; expired ones are `Expired`; unknown, empty, oversized and
///   wrong-kind tokens are `InvalidToken` without being parsed; a wrong audience is `WrongAudience`.
/// - No error mentions the token.
pub async fn token_verifier<V: TokenVerifier + ?Sized>(v: &V, f: &TokenFixtures) {
    let (entra, user) = &f.entra_ok;
    assert_eq!(
        &v.entra_access_token(entra).await.expect("good Entra token"),
        user
    );
    assert_eq!(
        v.entra_access_token(&f.entra_expired).await.unwrap_err(),
        AuthError::Expired
    );

    let (google, aud, identity) = &f.google_ok;
    assert_eq!(
        &v.google_id_token(google, aud).await.expect("good Google token"),
        identity
    );
    assert_eq!(
        v.google_id_token(google, "https://another.example.com")
            .await
            .unwrap_err(),
        AuthError::WrongAudience
    );

    let oversized = "a".repeat(MAX_TOKEN_BYTES + 1);
    for bad in ["", "garbage", "a.b.c", oversized.as_str()] {
        let e = v.entra_access_token(bad).await.unwrap_err();
        assert_eq!(e, AuthError::InvalidToken, "entra token {:?}", bad.len());
        let e = v.google_id_token(bad, aud).await.unwrap_err();
        assert_eq!(e, AuthError::InvalidToken, "google token {:?}", bad.len());
    }
    assert_eq!(
        v.google_id_token(entra, aud).await.unwrap_err(),
        AuthError::InvalidToken,
        "an Entra token is not a Google token"
    );
    assert_eq!(
        v.entra_access_token(google).await.unwrap_err(),
        AuthError::InvalidToken,
        "a Google token is not an Entra token"
    );

    for err in [
        AuthError::InvalidToken,
        AuthError::Expired,
        AuthError::WrongAudience,
    ] {
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains(entra.as_str()) && !shown.contains(google.as_str()));
    }
}

/// What the users conformance suite needs from the test harness.
#[async_trait]
pub trait UsersScenario: Users {
    /// Add or replace a user.
    async fn seed(&self, user: User);
}

/// - Unknown users are `None` from `get` and `UnknownUser` from `require_active`.
/// - Active users pass when their role is at least `min`; a lower role, or no role, is `InsufficientRole`.
/// - Pending and disabled users never pass, whatever their role (S4).
pub async fn users<U: UsersScenario + ?Sized>(u: &U) {
    let admin = active(1, Role::Admin);
    let viewer = active(2, Role::Viewer);
    let pending = user(3, Some(Role::Admin), UserStatus::Pending);
    let disabled = user(4, Some(Role::Admin), UserStatus::Disabled);
    let no_role = user(5, None, UserStatus::Active);
    for x in [&admin, &viewer, &pending, &disabled, &no_role] {
        u.seed(x.clone()).await;
    }

    assert_eq!(u.get(&user_id(99)).await.unwrap(), None);
    assert_eq!(u.get(&admin.id).await.unwrap().as_ref(), Some(&admin));
    assert_eq!(
        u.require_active(&user_id(99), Role::Viewer).await.unwrap_err(),
        AuthError::UnknownUser
    );

    assert_eq!(u.require_active(&admin.id, Role::Admin).await.unwrap(), admin);
    assert_eq!(u.require_active(&admin.id, Role::Viewer).await.unwrap(), admin);
    assert_eq!(u.require_active(&viewer.id, Role::Viewer).await.unwrap(), viewer);
    assert_eq!(
        u.require_active(&viewer.id, Role::Editor).await.unwrap_err(),
        AuthError::InsufficientRole
    );
    assert_eq!(
        u.require_active(&no_role.id, Role::Viewer).await.unwrap_err(),
        AuthError::InsufficientRole
    );
    for role in Role::ALL {
        assert_eq!(
            u.require_active(&pending.id, role).await.unwrap_err(),
            AuthError::Pending
        );
        assert_eq!(
            u.require_active(&disabled.id, role).await.unwrap_err(),
            AuthError::Disabled
        );
    }
}
