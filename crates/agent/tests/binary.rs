//! The `agent` binary as an operator sees it (T1): a bad deployment stops it at once, with a clear message on stderr and
//! a non-zero exit code, and the message never contains a value from the environment.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::process::{Command, Output};

use support::valid_env;

fn run(env: impl IntoIterator<Item = (String, String)>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent"))
        .env_clear()
        .envs(env)
        .output()
        .expect("the agent binary starts")
}

#[test]
fn binary_starts_with_a_valid_environment() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(valid_env(tmp.path()));
    assert!(
        out.status.success(),
        "{:?} {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
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
