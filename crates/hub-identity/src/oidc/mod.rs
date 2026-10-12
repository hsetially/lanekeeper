//! Sign-in against Entra: discovery, the key set, token verification, client authentication and the login flow.

pub mod client_auth;
pub mod discovery;
pub mod jwks;
pub mod jwt;
pub mod login;

pub use client_auth::ClientCredentials;
pub use discovery::{Discovery, DiscoveryCache};
pub use jwks::{JwksCache, JwksLocation, KeySet, RsaPublicKey};
pub use jwt::{Claims, JwtVerifier, Profile};
pub use login::{
    CallbackQuery, LoginSealer, LoginStart, OidcLogin, SealedLoginCookie, SignIn, valid_return_to,
};
