//! The real `agent` binary as a child process, for the tests and benchmarks that need it: an environment that parses, a
//! hub CA file, a kubeconfig that points at nothing, a free health port and a blocking `GET` of it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::test_ca::TestCa;
use super::valid_env;

/// A free loopback port for the health endpoint (found by binding and letting go; a collision is possible, not likely).
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A valid environment whose hub CA file exists, and whose health port is `port`. Everything lives under `root`.
pub fn running_env(root: &Path, port: u16) -> HashMap<String, String> {
    let ca = root.join("hub-ca.pem");
    std::fs::write(&ca, TestCa::new().pem()).unwrap();
    let nfs = root.join("nfs");
    std::fs::create_dir_all(&nfs).unwrap();
    let mut env = valid_env(&nfs);
    env.insert("LK_HUB_CA_FILE".to_owned(), ca.to_string_lossy().into_owned());
    env.insert("LK_HEALTH_ADDR".to_owned(), format!("127.0.0.1:{port}"));
    env
}

/// A kubeconfig that points at nothing: the client builds, every call fails, and the agent keeps trying to join.
pub fn write_kubeconfig(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("kubeconfig");
    std::fs::write(
        &path,
        "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster: {server: 'http://127.0.0.1:1'}\n\
         users:\n- name: u\n  user: {token: 'not-a-real-token'}\n\
         contexts:\n- name: x\n  context: {cluster: c, user: u, namespace: sit1}\ncurrent-context: x\n",
    )
    .unwrap();
    path
}

/// Start `exe` (the agent binary) waiting for a certificate it cannot get. Returns the child and its health port.
pub fn spawn_waiting_agent(exe: &str, dir: &Path) -> (Child, u16) {
    let port = free_port();
    let env = running_env(dir, port);
    let kubeconfig = write_kubeconfig(dir);
    let child = Command::new(exe)
        .env_clear()
        .envs(env)
        .env("KUBECONFIG", &kubeconfig)
        .env("HOME", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the agent binary starts");
    (child, port)
}

/// A blocking `GET` of `path` on `port`: the status code, and the body.
pub fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    let code = reply.split_whitespace().nth(1)?.parse().ok()?;
    let body = reply.split("\r\n\r\n").nth(1)?.to_owned();
    Some((code, body))
}

/// Wait (in real time) until the health port answers `/healthz`.
pub fn wait_for_health(port: u16) {
    for _ in 0..100 {
        if http_get(port, "/healthz").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the agent never opened its health port");
}

/// Send SIGTERM, the signal the kubelet sends.
pub fn terminate(child: &Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}
