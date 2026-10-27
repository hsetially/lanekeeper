//! The development sign-in of `just dev` (plan 18, T8, G23). NOT for production: nothing under `src/` names this file, and the real
//! binary has no such thing.
//!
//! A browser cannot sign in to the hub against a mock Entra: the discovery check requires the authorize endpoint to be
//! `https://<authority host>/...` with no port, which a laptop cannot serve. So the example offers a shortcut that is honest about what
//! it is. When `LK_DEV_LOGIN_ADDR` is set (a loopback address, such as `127.0.0.1:8480`), a thread of the example listens there, and a
//! `GET /` makes a development administrator (a row in `users`, the way a test makes one) and a session for it **through the hub's own
//! `Sessions`**, and answers with the two cookies of a signed-in page and a redirect to the hub's public origin. The hub itself finds
//! the session in the table as it finds any other: nothing in the hub knows this exists, so what it shows is what a real
//! administrator would see.
//!
//! The listener serves nobody else: it refuses an address that is not loopback at start, and a request whose `Host` is not a
//! loopback name (so a web page cannot use DNS rebinding to sign a browser in), and it answers `GET /` only. Cookies are scoped to a
//! host name and not a port, so the page to open is `http://localhost:<port>/` (the hub's public origin is `https://localhost:8443`):
//! whether a browser accepts a `Secure` cookie from `http://localhost` is the browser's decision (Chrome and Firefox treat it as a
//! secure context); `curl -i http://localhost:<port>/` prints the same two lines for a person to set by hand.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use domain::{Guid, Role, UserId, UserStatus};
use hub::config::HubSettings;
use hub::edges::IdentityClock;
use hub_identity::session::cookie::{csrf_set_cookie, session_set_cookie};
use hub_identity::session::{SessionPolicy, Sessions};
use hub_platform::clock::SystemClock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The development administrator's object id. Fixed, so that the same person is signed in at every start.
const DEV_ADMIN_OID: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const REQUEST_LIMIT: usize = 8 * 1024;
const REQUEST_TIME: Duration = Duration::from_secs(5);

/// Start the listener on its own thread, if `LK_DEV_LOGIN_ADDR` names one. Called before `hub::process::entry`; it makes no
/// connection to the database until the first request.
pub fn spawn_if_asked(lookup: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
    let Some(addr) = lookup("LK_DEV_LOGIN_ADDR") else {
        return Ok(());
    };
    let addr: SocketAddr = addr
        .parse()
        .map_err(|_| "LK_DEV_LOGIN_ADDR is not an address".to_owned())?;
    if !addr.ip().is_loopback() {
        return Err("LK_DEV_LOGIN_ADDR must be a loopback address".to_owned());
    }
    let settings = HubSettings::from_lookup(lookup).map_err(|e| e.to_string())?;
    if settings.database.is_none() || settings.identity.is_none() {
        return Err("LK_DEV_LOGIN_ADDR needs the database and the Entra settings".to_owned());
    }
    // Bind now, so that an address that is taken is an error at start and not a thread that died.
    let listener =
        std::net::TcpListener::bind(addr).map_err(|_| "LK_DEV_LOGIN_ADDR cannot be bound".to_owned())?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "LK_DEV_LOGIN_ADDR cannot be used".to_owned())?;
    std::thread::Builder::new()
        .name("lk-dev-login".to_owned())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            runtime.block_on(async move {
                let Ok(listener) = TcpListener::from_std(listener) else {
                    return;
                };
                serve(listener, settings).await;
            });
        })
        .map_err(|_| "the dev login thread could not be started".to_owned())?;
    Ok(())
}

async fn serve(listener: TcpListener, settings: HubSettings) {
    let (Some(database), Some(identity)) = (settings.database.as_ref(), settings.identity.as_ref()) else {
        return;
    };
    let origin = identity.public_origin();
    let tenant = identity.config.entra.tenant_id.clone();
    let mut pool = None;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        // One at a time: this is a development convenience, and a request that stalls is cut off.
        let _ = tokio::time::timeout(REQUEST_TIME, async {
            if pool.is_none() {
                pool = hub::db::connect(database).await.ok();
            }
            if let Some(pool) = &pool {
                respond(stream, pool, &tenant, &origin).await;
            }
        })
        .await;
    }
}

/// `Host: localhost:8480`, `127.0.0.1` and `[::1]` are loopback names; nothing else is believed.
fn host_is_loopback(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default()
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

async fn respond(mut stream: TcpStream, pool: &sqlx::PgPool, tenant: &Guid, origin: &str) {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < REQUEST_LIMIT {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&chunk[..n]),
        }
    }
    let text = String::from_utf8_lossy(&request);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let host = lines
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("host").then(|| value.trim().to_owned())
        })
        .unwrap_or_default();
    let reply = if !host_is_loopback(&host) {
        plain(
            421,
            "Misdirected Request",
            "this listener answers loopback names only\n",
        )
    } else if first.split_whitespace().take(2).collect::<Vec<_>>() != ["GET", "/"] {
        plain(404, "Not Found", "GET / signs the development administrator in\n")
    } else {
        match sign_in(pool, tenant, origin).await {
            Some(reply) => reply,
            None => plain(503, "Service Unavailable", "the database did not answer\n"),
        }
    };
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn plain(status: u16, reason: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn sign_in(pool: &sqlx::PgPool, tenant: &Guid, origin: &str) -> Option<String> {
    let id = UserId::new(tenant.clone(), Guid::parse(DEV_ADMIN_OID).ok()?);
    sqlx::query(
        "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (tid, oid) DO UPDATE SET role = EXCLUDED.role, status = EXCLUDED.status",
    )
    .bind(id.tid().as_str())
    .bind(id.oid().as_str())
    .bind("dev-admin@example.com")
    .bind("Development administrator")
    .bind(Role::Admin.as_str())
    .bind(UserStatus::Active.as_str())
    .execute(pool)
    .await
    .ok()?;
    let policy = SessionPolicy::default();
    let max_age = i64::try_from(policy.absolute.as_secs()).ok()?;
    let sessions = Sessions::new(
        pool.clone(),
        Arc::new(IdentityClock(Arc::new(SystemClock))),
        &policy,
    );
    let mut tx = pool.begin().await.ok()?;
    let made = sessions.store().create_in(&mut tx, &id).await.ok()?;
    tx.commit().await.ok()?;
    Some(format!(
        "HTTP/1.1 302 Found\r\nLocation: {origin}/\r\nSet-Cookie: {}\r\nSet-Cookie: {}\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        session_set_cookie(&made.session, max_age),
        csrf_set_cookie(&made.csrf, max_age),
    ))
}
