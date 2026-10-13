//! Security headers on every web response (T1, S12).
//!
//! Every response, errors included, carries HSTS, `nosniff`, `Referrer-Policy: no-referrer`, COOP and CORP `same-origin`
//! and a `Permissions-Policy` that denies the sensitive features. What differs by class:
//!
//! | Class | CSP | `Cache-Control` |
//! |---|---|---|
//! | HTML (`index.html`) | full, with this response's nonce, Trusted Types required | `no-store` |
//! | hashed static asset (`/assets/…`) | API policy | `public, max-age=31536000, immutable` |
//! | blob (`/api/v1/blobs/…`) | API policy | the handler's `private, … immutable` on success |
//! | API, SSE, problems, everything else | API policy | `no-store` |
//!
//! `immutable` is allowed only on a successful asset or blob response; on anything else it is replaced by `no-store`, so a
//! handler that asks for a long cache on mutable data is overruled here and not trusted to remember.
//!
//! The nonce is 128 random bits from the system CSPRNG, new for every response. It goes into the request's extensions so
//! that the static handler puts the same value into `index.html` that the header names. The policy has no `unsafe-inline`,
//! no `unsafe-eval`, no `strict-dynamic` and no origin but `'self'`; Monaco's needs, if any, are recorded by prompt 08 in
//! `design/DEVIATIONS.md`, never relaxed here.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http::{HeaderName, HeaderValue, header};

use crate::random::random_hex;

/// The nonce of one response, in the request extensions so `index.html` can carry the same one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CspNonce(pub String);

/// What the header layer needs to know.
#[derive(Debug, Clone)]
pub struct HeaderPolicy {
    /// The Trusted Types policy names the web app creates (A14). Validated when the settings are read.
    pub trusted_types: Vec<String>,
}

/// What kind of response this is, for the headers that differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteClass {
    Html,
    StaticAsset,
    Blob,
    /// JSON, problems, SSE, webhooks and everything else.
    Api,
}

/// The class from the request path and the response's content type.
#[must_use]
pub fn classify(path: &str, content_type: Option<&str>) -> RouteClass {
    let media = content_type.map(|c| c.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
    if media.as_deref() == Some("text/html") {
        RouteClass::Html
    } else if path.starts_with("/api/v1/blobs/") {
        RouteClass::Blob
    } else if path.starts_with("/assets/") {
        RouteClass::StaticAsset
    } else {
        RouteClass::Api
    }
}

const HSTS: &str = "max-age=63072000; includeSubDomains";

/// Every sensitive browser feature is denied. `clipboard-write` is left at its default (`self`) so the app's copy buttons
/// work; nothing else is allowed.
const PERMISSIONS: &str = "accelerometer=(), autoplay=(), bluetooth=(), camera=(), display-capture=(), \
encrypted-media=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), \
publickey-credentials-get=(), screen-wake-lock=(), serial=(), usb=(), xr-spatial-tracking=()";

/// A response that is not a page is never framed, never loads anything and never submits anywhere.
const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// The policy of the app's own page.
#[must_use]
pub fn html_csp(nonce: &str, trusted_types: &[String]) -> String {
    let mut csp = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; \
img-src 'self' data:; font-src 'self'; connect-src 'self'; worker-src 'self'; frame-ancestors 'none'; \
base-uri 'none'; form-action 'self'; object-src 'none'; require-trusted-types-for 'script'"
    );
    if !trusted_types.is_empty() {
        csp.push_str("; trusted-types ");
        csp.push_str(&trusted_types.join(" "));
    }
    csp
}

const NO_STORE: &str = "no-store";

fn set(response: &mut Response, name: HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

/// Apply the headers of `class` to `response`.
pub fn apply(response: &mut Response, class: RouteClass, policy: &HeaderPolicy, nonce: Option<&str>) {
    let status = response.status().as_u16();
    let success = (200..300).contains(&status) || status == 304;
    let immutable_allowed = success && matches!(class, RouteClass::StaticAsset | RouteClass::Blob);
    let handler_says_immutable = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("immutable"));
    if !(immutable_allowed && handler_says_immutable) {
        set(response, header::CACHE_CONTROL, NO_STORE);
    }

    let csp = match (class, nonce) {
        (RouteClass::Html, Some(nonce)) => html_csp(nonce, &policy.trusted_types),
        // An HTML page without a nonce cannot run any script, which is the safe failure.
        _ => API_CSP.to_owned(),
    };
    set(response, header::CONTENT_SECURITY_POLICY, &csp);
    set(response, header::STRICT_TRANSPORT_SECURITY, HSTS);
    set(response, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(response, header::REFERRER_POLICY, "no-referrer");
    set(
        response,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    set(
        response,
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );
    set(
        response,
        HeaderName::from_static("permissions-policy"),
        PERMISSIONS,
    );
}

/// The layer: make a nonce, let the inner layers and the handler use it, then set the headers on whatever came back.
pub async fn security_headers(
    State(policy): State<Arc<HeaderPolicy>>,
    mut request: Request,
    next: Next,
) -> Response {
    let nonce = random_hex(16).ok();
    if let Some(nonce) = &nonce {
        request.extensions_mut().insert(CspNonce(nonce.clone()));
    }
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    let class = classify(
        &path,
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    );
    apply(&mut response, class, &policy, nonce.as_deref());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_come_from_the_content_type_first_then_the_path() {
        assert_eq!(classify("/", Some("text/html; charset=utf-8")), RouteClass::Html);
        assert_eq!(
            classify("/api/v1/blobs/abc", Some("application/octet-stream")),
            RouteClass::Blob
        );
        assert_eq!(
            classify("/assets/a.js", Some("text/javascript")),
            RouteClass::StaticAsset
        );
        assert_eq!(
            classify("/api/v1/ping", Some("application/json")),
            RouteClass::Api
        );
        assert_eq!(
            classify("/api/v1/events", Some("text/event-stream")),
            RouteClass::Api
        );
        assert_eq!(classify("/x", None), RouteClass::Api);
    }

    #[test]
    fn the_html_policy_names_the_nonce_twice_and_the_trusted_types() {
        let csp = html_csp("abc", &["a".to_owned(), "b".to_owned()]);
        assert_eq!(csp.matches("'nonce-abc'").count(), 2);
        assert!(csp.ends_with("trusted-types a b"));
        assert!(
            !html_csp("abc", &[]).contains("; trusted-types "),
            "no policy names, no trusted-types directive"
        );
    }
}
