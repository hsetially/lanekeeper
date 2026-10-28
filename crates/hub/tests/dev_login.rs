//! The development sign-in of `just dev` (plan 18, T8, G23): `examples/support/dev_login.rs`. It is a convenience of an example and
//! not part of the hub, and these tests hold three things about it. It signs a development administrator in through the hub's own
//! sessions, so the cookies it hands out work on a real route of the real hub. It refuses what it should (a `Host` that is not a
//! loopback name, any path but `/`, an address that is not loopback). And it is absent unless asked for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

#[path = "support/child.rs"]
mod child;
#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/net.rs"]
mod net;
#[path = "support/rig.rs"]
mod rig;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::net::SocketAddr;
use std::time::Duration;

use rig::Rig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A raw HTTP/1.1 request with the `Host` the test names (the client of `net` always says `localhost`).
async fn raw(addr: SocketAddr, request: &str) -> net::Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut text = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut text)).await;
    let text = String::from_utf8_lossy(&text).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    net::Reply {
        status,
        headers: lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
            .collect(),
        body: body.to_owned(),
    }
}

fn cookie_value(reply: &net::Reply, name: &str) -> String {
    reply
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .find_map(|(_, v)| v.strip_prefix(&format!("{name}=")))
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("no {name} cookie in {:?}", reply.headers))
        .to_owned()
}

#[tokio::test]
async fn the_dev_login_signs_an_administrator_in_through_the_hubs_own_sessions() {
    let Some(rig) = Rig::start().await else { return };
    let port = free_loopback_port();
    let login = format!("127.0.0.1:{port}");
    let mut hub = rig.spawn(&[("LK_DEV_LOGIN_ADDR", &login)]).await;
    let (web, ops) = hub.wait_listening(Duration::from_secs(30));
    let addr: SocketAddr = login.parse().unwrap();
    for _ in 0..100 {
        if net::http_get(ops, "/readyz").await.status == 200 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // `GET /` signs in and sends the browser to the hub's public origin, with the two cookies a signed-in page holds.
    let reply = raw(
        addr,
        &format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(reply.status, 302, "{reply:?}");
    assert_eq!(reply.header("location"), Some("https://localhost/"));
    let lines: Vec<&str> = reply
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("__Host-lk_session=") && l.contains("; Secure; HttpOnly; SameSite=Lax"))
    );
    assert!(lines.iter().any(|l| l.starts_with("__Host-lk_csrf=")
        && l.contains("; Secure; SameSite=Lax")
        && !l.contains("HttpOnly")));
    let (session, csrf) = (
        cookie_value(&reply, "__Host-lk_session"),
        cookie_value(&reply, "__Host-lk_csrf"),
    );

    // The cookies work on the real hub: `me` is the development administrator, and a route only an administrator may use answers.
    let cookie = format!("__Host-lk_session={session}; __Host-lk_csrf={csrf}");
    let ca = rig.ca.cert_der();
    let me = net::https_request(web, &ca, "GET", "/api/v1/me", &[("Cookie", &cookie)], None).await;
    assert_eq!(me.status, 200, "{me:?}");
    let me: serde_json::Value = serde_json::from_str(&me.body).unwrap();
    assert_eq!(me["role"], "admin", "{me}");
    let users = net::https_request(
        web,
        &ca,
        "GET",
        "/api/v1/admin/users",
        &[("Cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(users.status, 200, "{users:?}");

    // A second visit makes a second session for the same person, and the first one is still good.
    let again = raw(
        addr,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(again.status, 302);
    assert_ne!(cookie_value(&again, "__Host-lk_session"), session);
    let still = net::https_request(web, &ca, "GET", "/api/v1/me", &[("Cookie", &cookie)], None).await;
    assert_eq!(still.status, 200);

    // What it refuses: a Host that is not a loopback name (DNS rebinding), a path that is not `/`, and a method that is not GET.
    for (what, request) in [
        (
            "a foreign host",
            format!("GET / HTTP/1.1\r\nHost: evil.example:{port}\r\nConnection: close\r\n\r\n"),
        ),
        (
            "no host",
            "GET / HTTP/1.1\r\nConnection: close\r\n\r\n".to_owned(),
        ),
        (
            "another path",
            format!("GET /admin HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"),
        ),
        (
            "a POST",
            format!(
                "POST / HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        ),
    ] {
        let reply = raw(addr, &request).await;
        assert!(matches!(reply.status, 404 | 421), "{what}: {reply:?}");
        assert!(
            !reply
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("set-cookie")),
            "{what} was given a cookie"
        );
    }

    // The hub's output holds no cookie value.
    let output = hub.output();
    assert!(!output.contains(&session) && !output.contains(&csrf));
    hub.signal("TERM");
    assert!(hub.exit_within(Duration::from_secs(30)).success());
}

#[tokio::test]
async fn the_dev_login_is_off_unless_asked_for_and_refuses_an_address_that_is_not_loopback() {
    let Some(rig) = Rig::start().await else { return };
    // Not asked for: nothing listens on the usual port, and the hub is up all the same.
    let mut hub = rig.spawn(&[]).await;
    let (_, ops) = hub.wait_listening(Duration::from_secs(30));
    assert_eq!(net::http_get(ops, "/healthz").await.status, 200);
    hub.signal("TERM");
    assert!(hub.exit_within(Duration::from_secs(30)).success());

    // An address that is not loopback is exit 2 with a message that names the variable, before the hub starts.
    let mut refused = rig.spawn(&[("LK_DEV_LOGIN_ADDR", "0.0.0.0:8480")]).await;
    let status = refused.exit_within(Duration::from_secs(30));
    assert_eq!(status.code(), Some(2), "{}", refused.output());
    assert!(
        refused
            .output()
            .contains("LK_DEV_LOGIN_ADDR must be a loopback address")
    );
    assert!(
        !refused.output().contains("0.0.0.0"),
        "the message repeats a value"
    );
}

#[tokio::test]
async fn the_dev_login_refuses_a_database_that_is_not_on_this_machine() {
    let Some(rig) = Rig::start().await else { return };
    // The shortcut signs an administrator in to whatever database the hub is pointed at, so it must never be pointed at a shared one:
    // a host that is not a loopback name or address is exit 2, before the hub starts, and the message repeats neither the host nor the
    // password. Names that only look local (a loopback name as a prefix of a real one) are not local.
    let port = free_loopback_port();
    let login = format!("127.0.0.1:{port}");
    let dir = tempfile::tempdir().unwrap();
    let url = dir.path().join("db-url");
    for host in [
        "db.example.test",
        "10.0.0.5",
        "localhost.evil.example",
        "127.0.0.1.evil.example",
        "0.0.0.0",
    ] {
        std::fs::write(
            &url,
            format!("postgres://lanekeeper_app:planted-secret-4821@{host}:5432/lanekeeper"),
        )
        .unwrap();
        let mut refused = rig
            .spawn(&[
                ("LK_DEV_LOGIN_ADDR", &login),
                ("LK_DATABASE_URL_FILE", url.to_str().unwrap()),
            ])
            .await;
        let status = refused.exit_within(Duration::from_secs(30));
        let output = refused.output();
        assert_eq!(status.code(), Some(2), "{host}: {output}");
        assert!(
            output.contains("LK_DEV_LOGIN_ADDR needs a database on this machine"),
            "{host}: {output}"
        );
        assert!(
            !output.contains(host) && !output.contains("planted-secret-4821"),
            "the message repeats a value: {output}"
        );
    }
}
