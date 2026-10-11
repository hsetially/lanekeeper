//! A small web app around the real `AuthLayer` and `CsrfLayer`: a few stand-in handlers behind them, driven with
//! `tower::ServiceExt::oneshot`, so the tests send real HTTP requests through the real layers and a real database.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use domain::{RequestId, Role, UserId};
use hub_identity::csrf::{CsrfExemption, CsrfLayer, CsrfPolicy};
use hub_identity::middleware::{AuthContext, AuthLayer};
use hub_identity::session::Sessions;
use tower::ServiceExt;

/// The public origin the layers are configured with (the mock Entra's).
pub const ORIGIN: &str = "https://lanekeeper.example.com";

/// What a stand-in handler saw: the context the auth layer injected, if any.
async fn whoami(req: Request) -> Response {
    let request_id = req.extensions().get::<RequestId>().map(ToString::to_string);
    let body = match req.extensions().get::<AuthContext>() {
        None => serde_json::json!({"anonymous": true, "request_id": request_id}),
        Some(ctx) => serde_json::json!({
            "anonymous": false,
            "user": ctx.user.to_string(),
            "role": ctx.role.map(Role::as_str),
            "status": ctx.status.as_str(),
            "request_id": request_id,
        }),
    };
    (StatusCode::OK, body.to_string()).into_response()
}

/// A state-changing stand-in. Reaching it means every layer in front of it let the request through.
async fn changed() -> Response {
    (StatusCode::NO_CONTENT, "").into_response()
}

pub fn app(sessions: &Arc<Sessions>) -> Router {
    Router::new()
        .route("/whoami", get(whoami))
        .route(
            "/change",
            post(changed)
                .put(changed)
                .patch(changed)
                .delete(changed)
                .get(whoami),
        )
        .layer(CsrfLayer::new(ORIGIN, CsrfPolicy::Required))
        .layer(AuthLayer::new(Arc::clone(sessions)))
}

/// The same handlers with the CSRF check turned off for a route that is authenticated another way (an HMAC-signed hook).
pub fn app_with_exempt_hook(sessions: &Arc<Sessions>) -> Router {
    let exempt =
        CsrfExemption::new("test hook: authenticated by an HMAC over the body, not by a cookie").unwrap();
    let hook = Router::new()
        .route("/hook", post(changed))
        .layer(CsrfLayer::new(ORIGIN, CsrfPolicy::Exempt(exempt)));
    Router::new()
        .route("/change", post(changed))
        .layer(CsrfLayer::new(ORIGIN, CsrfPolicy::Required))
        .merge(hook)
        .layer(AuthLayer::new(Arc::clone(sessions)))
}

/// A request builder with the headers the tests vary.
pub struct Req {
    method: Method,
    path: String,
    cookies: Vec<String>,
    headers: Vec<(&'static str, String)>,
}

impl Req {
    pub fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            cookies: Vec::new(),
            headers: Vec::new(),
        }
    }

    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn post(path: &str) -> Self {
        Self::new(Method::POST, path)
    }

    pub fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.push(format!("{name}={value}"));
        self
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }

    pub fn origin(self, value: &str) -> Self {
        self.header("origin", value)
    }

    /// A browser's state-changing request with a session: both cookies, the header, and the right `Origin`.
    pub fn with_session(self, session: &str, csrf: &str) -> Self {
        self.cookie("__Host-lk_session", session)
            .cookie("__Host-lk_csrf", csrf)
            .header("x-csrf-token", csrf)
            .origin(ORIGIN)
    }

    pub fn build(self) -> Request<Body> {
        let mut builder = Request::builder().method(self.method).uri(self.path);
        if !self.cookies.is_empty() {
            builder = builder.header(header::COOKIE, self.cookies.join("; "));
        }
        for (name, value) in self.headers {
            builder = builder.header(name, HeaderValue::from_str(&value).unwrap());
        }
        builder.body(Body::empty()).unwrap()
    }
}

/// The answer, read whole.
pub struct Answer {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: String,
}

impl Answer {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect()
    }
}

pub async fn send(app: &Router, req: Req) -> Answer {
    let response = app.clone().oneshot(req.build()).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024).await.unwrap();
    Answer {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// The user a `whoami` answer names.
pub fn whoami_user(a: &Answer) -> Option<UserId> {
    a.json()["user"].as_str().and_then(|s| s.parse().ok())
}

// ---------------------------------------------------------------------------------------------------------------
// The sign-in routes behind the real layers, wired to the mock Entra

use std::collections::BTreeSet;

use domain::Timestamp;
use hub_identity::Clock;
use hub_identity::oidc::login::OidcLogin;
use hub_identity::routes::auth::{AuthState, auth_routes};
use hub_identity::users::PgUsers;
use ports::{AuditLog, EventBus};

use super::fake_https::FakeHttps;
use super::login_rig::Begun;
use super::mock_oidc::{MockOidc, NOW_MS, OID, TID, TokenSpec, config, secret_auth, secrets};
use super::sessions_rig::Replica;
use super::simple_audit::SimpleChainAudit;
use super::users_rig::UsersRig;

/// Everything for the sign-in tests: one replica, the mock Entra, the routes and the stand-in handlers behind the layers.
pub struct RoutesRig {
    pub users: UsersRig,
    pub replica: Replica,
    pub mock: Arc<MockOidc>,
    pub https: Arc<FakeHttps>,
    pub app: Router,
    pub state: AuthState,
}

/// The identity the mock Entra's default token names.
pub fn mock_user() -> UserId {
    UserId::new(TID.parse().unwrap(), OID.parse().unwrap())
}

impl RoutesRig {
    pub async fn new(db: super::pg::TestDb) -> Self {
        Self::with_audit(db, Arc::new(SimpleChainAudit)).await
    }

    /// `audit` is what the sign-in and logout transactions record to; the user directory keeps the normal one.
    pub async fn with_audit(db: super::pg::TestDb, audit: Arc<dyn AuditLog>) -> Self {
        let users = UsersRig::new(db).await;
        // The mock's tokens are valid around `NOW_MS`.
        users.clock.set(Timestamp::from_unix_millis(NOW_MS));
        let replica = Replica::start(&users.db, &users.clock).await;
        let mock = MockOidc::new();
        let https = mock.https();
        let clock: Arc<dyn Clock> = Arc::clone(&users.clock) as Arc<dyn Clock>;
        let login = OidcLogin::new(
            config(secret_auth()),
            Arc::clone(&https) as Arc<_>,
            secrets(),
            Arc::clone(&clock),
        )
        .await
        .unwrap();
        let svc = hub_identity::users::UserServices {
            pool: replica.pool.clone(),
            bus: Arc::clone(&replica.bus) as Arc<_>,
            ..users.svc.clone()
        };
        let state = AuthState::new(
            login,
            PgUsers::new(svc, BTreeSet::new()),
            Arc::clone(&replica.sessions),
            replica.pool.clone(),
            audit,
            Arc::clone(&replica.bus) as Arc<dyn EventBus>,
            clock,
        );
        let app = Router::new()
            .merge(auth_routes(state.clone()))
            .route("/whoami", get(whoami))
            .layer(CsrfLayer::new(ORIGIN, CsrfPolicy::Required))
            .layer(AuthLayer::new(Arc::clone(&replica.sessions)));
        Self {
            users,
            replica,
            mock,
            https,
            app,
            state,
        }
    }

    /// Insert the mock identity as a known user.
    pub async fn seed_mock_user(&self, role: Option<Role>, status: domain::UserStatus) {
        sqlx::query("INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, 'ada@example.com', 'Ada', $3, $4)")
            .bind(TID)
            .bind(OID)
            .bind(role.map(Role::as_str))
            .bind(status.as_str())
            .execute(&self.replica.pool)
            .await
            .unwrap();
    }

    /// `GET /auth/login`, taken apart.
    pub async fn begin(&self, return_to: Option<&str>) -> Begun {
        let path = match return_to {
            Some(r) => format!(
                "/auth/login?return_to={}",
                url::form_urlencoded::byte_serialize(r.as_bytes()).collect::<String>()
            ),
            None => "/auth/login".to_owned(),
        };
        let a = send(&self.app, Req::get(&path)).await;
        assert_eq!(a.status, StatusCode::FOUND, "{}", a.body);
        let location = a
            .headers
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let redirect = url::Url::parse(&location).unwrap();
        let params = redirect
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let cookie = cookie_value(&a.set_cookies(), "__Host-lk_login").expect("a login cookie");
        Begun {
            redirect,
            params,
            cookie,
        }
    }

    /// `GET /auth/callback` for a login that `begin` started; the mock returns the token of `spec`.
    pub async fn callback(&self, begun: &Begun, spec: TokenSpec, extra_cookies: &[(&str, &str)]) -> Answer {
        self.mock.set_id_token(spec.mint());
        self.callback_raw(
            &format!("code=the-auth-code&state={}", begun.param("state")),
            Some(&begun.cookie),
            extra_cookies,
        )
        .await
    }

    pub async fn callback_raw(
        &self,
        query: &str,
        login_cookie: Option<&str>,
        extra_cookies: &[(&str, &str)],
    ) -> Answer {
        let mut req = Req::get(&format!("/auth/callback?{query}"));
        if let Some(c) = login_cookie {
            req = req.cookie("__Host-lk_login", c);
        }
        for (name, value) in extra_cookies {
            req = req.cookie(name, value);
        }
        send(&self.app, req).await
    }

    /// A whole sign-in as the mock identity, with the token the mock mints by default.
    pub async fn sign_in(&self, extra_cookies: &[(&str, &str)]) -> Answer {
        let begun = self.begin(Some("/swimlanes")).await;
        let spec = TokenSpec::valid(begun.param("nonce"));
        self.callback(&begun, spec, extra_cookies).await
    }
}

/// The value of a cookie in a list of `Set-Cookie` lines.
pub fn cookie_value(set_cookies: &[String], name: &str) -> Option<String> {
    set_cookies
        .iter()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .map(|rest| rest.split(';').next().unwrap_or_default().to_owned())
}

/// The whole `Set-Cookie` line of a cookie.
pub fn cookie_line<'a>(set_cookies: &'a [String], name: &str) -> Option<&'a str> {
    set_cookies
        .iter()
        .map(String::as_str)
        .find(|line| line.starts_with(&format!("{name}=")))
}
