//! The `agent` binary as an operator sees it (T1): a bad deployment stops it at once, with a clear message on stderr and
//! a non-zero exit code, and the message never contains a value from the environment.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::process::{Command, Output};

use support::agent_process::{
    free_port, http_get, running_env, spawn_waiting_agent, terminate, wait_for_health,
};
use support::valid_env;

fn run(env: impl IntoIterator<Item = (String, String)>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent"))
        .env_clear()
        .envs(env)
        .output()
        .expect("the agent binary starts")
}

#[test]
fn binary_stops_with_a_clear_error_outside_a_cluster() {
    let tmp = tempfile::tempdir().unwrap();
    let env = running_env(tmp.path(), free_port());
    let out = Command::new(env!("CARGO_BIN_EXE_agent"))
        .env_clear()
        .envs(env)
        // No service account and no kubeconfig: the agent cannot read its certificate Secret.
        .env("HOME", tmp.path())
        .output()
        .expect("the agent binary starts");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(log.contains("Kubernetes client"), "{log}");
    // Every line it logged is a JSON object.
    for line in log.lines() {
        assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
    }
}

#[test]
fn binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut child, port) = spawn_waiting_agent(env!("CARGO_BIN_EXE_agent"), tmp.path());

    // The port opens first, whatever else is not ready.
    wait_for_health(port);
    assert_eq!(http_get(port, "/healthz"), Some((200, "ok\n".to_owned())));
    assert_eq!(
        http_get(port, "/readyz"),
        Some((503, "not ready: starting\n".to_owned())),
        "alive, and not ready until it has a certificate and a connection"
    );
    let (code, metrics) = http_get(port, "/metrics").unwrap();
    assert_eq!(code, 200);
    assert!(metrics.contains("lanekeeper_agent_connection_state"));

    // SIGTERM is what the kubelet sends.
    terminate(&child);
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the agent did not stop within 20 s of SIGTERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0), "an orderly shutdown exits 0");
}

#[test]
fn binary_exits_with_a_clear_error_on_a_missing_environment() {
    let out = run(std::iter::empty());
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot start") && stderr.contains("is not set"),
        "{stderr}"
    );
    assert!(stderr.contains("LK_"), "the message names the variable: {stderr}");
}

#[test]
fn binary_exits_with_a_clear_error_on_a_missing_mount() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(valid_env(&tmp.path().join("not-mounted")));
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not-mounted") && stderr.contains("mounted"),
        "{stderr}"
    );
}

#[test]
fn binary_errors_never_print_environment_values() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = valid_env(tmp.path());
    // A secret pasted into the wrong variable must not come back out in the error.
    env.insert(
        "LK_HUB_ENDPOINT".to_owned(),
        "zq-planted-secret-value-wx".to_owned(),
    );
    let out = run(env);
    assert_eq!(out.status.code(), Some(2));
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(output.contains("LK_HUB_ENDPOINT"), "{output}");
    assert!(!output.contains("zq-planted-secret-value-wx"), "{output}");
}
