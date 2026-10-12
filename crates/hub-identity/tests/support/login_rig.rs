//! A login rig: the code under test wired to the mock Entra, with the clock and the secrets under the test's control.

use std::collections::HashMap;
use std::sync::Arc;

use hub_identity::config::ClientAuth;
use hub_identity::oidc::login::{CallbackQuery, OidcLogin, SignIn};
use hub_identity::{IdentityError, ManualClock};
use url::Url;

use super::fake_https::FakeHttps;
use super::mock_oidc::{MockOidc, NOW_MS, TokenSpec, config, secret_auth, secrets};

pub struct Rig {
    pub mock: Arc<MockOidc>,
    pub https: Arc<FakeHttps>,
    pub clock: Arc<ManualClock>,
    pub login: OidcLogin,
}

pub async fn rig() -> Rig {
    rig_with(secret_auth()).await
}

pub async fn rig_with(auth: ClientAuth) -> Rig {
    let mock = MockOidc::new();
    let https = mock.https();
    let clock = Arc::new(ManualClock::at(domain::Timestamp::from_unix_millis(NOW_MS)));
    let login = OidcLogin::new(
        config(auth),
        Arc::clone(&https) as Arc<_>,
        secrets(),
        Arc::clone(&clock) as Arc<_>,
    )
    .await
    .unwrap();
    Rig {
        mock,
        https,
        clock,
        login,
    }
}

/// What `begin` produced, taken apart.
pub struct Begun {
    pub redirect: Url,
    pub params: HashMap<String, String>,
    pub cookie: String,
}

impl Begun {
    pub fn param(&self, name: &str) -> &str {
        self.params.get(name).map_or("", String::as_str)
    }
}

pub async fn begin(rig: &Rig, return_to: Option<&str>) -> Begun {
    let start = rig.login.begin(return_to).await.unwrap();
    let params = start
        .redirect_to
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    Begun {
        redirect: start.redirect_to,
        params,
        cookie: start.cookie.as_str().to_owned(),
    }
}

pub fn query(code: &str, state: &str) -> CallbackQuery {
    CallbackQuery::parse(&format!("code={code}&state={state}")).unwrap()
}

/// Begin a login, let the mock return the token that `spec` builds from the login's nonce, and finish it.
pub async fn sign_in_with(rig: &Rig, spec: impl FnOnce(&str) -> TokenSpec) -> Result<SignIn, IdentityError> {
    let begun = begin(rig, Some("/swimlanes")).await;
    rig.mock.set_id_token(spec(begun.param("nonce")).mint());
    rig.login
        .complete(&query("the-auth-code", begun.param("state")), Some(&begun.cookie))
        .await
}
