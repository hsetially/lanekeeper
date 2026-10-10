//! Startup against a temporary directory (T1): the NFS root is opened once as a cap-std handle, and a bad mount or a
//! bad environment stops the agent with a clear error before it does anything else.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::io::Read;

use agent::config::SettingsError;
use agent::root::StartupError;
use support::valid_env;

#[test]
fn startup_opens_cap_std_root() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("app")).unwrap();
    std::fs::write(tmp.path().join("app/a.yml"), b"key: value\r\n").unwrap();

    let started = agent::startup(&valid_env(tmp.path())).unwrap();

    assert_eq!(started.settings.swimlane.as_str(), "sit1");
    assert_eq!(started.root.path(), tmp.path());
    // The handle reads inside the root...
    let mut bytes = Vec::new();
    started
        .root
        .dir()
        .open("app/a.yml")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"key: value\r\n");
    // ...and is a capability: it cannot name anything outside it, by `..` or by an absolute path.
    assert!(started.root.dir().open("../outside").is_err());
    assert!(started.root.dir().open("app/../../outside").is_err());
    assert!(started.root.dir().open("/etc/passwd").is_err());
}

#[test]
fn an_empty_export_is_a_valid_start() {
    // Telling "empty" from "unmounted" is the scanner's job (T4), not the startup check's.
    let tmp = tempfile::tempdir().unwrap();
    agent::startup(&valid_env(tmp.path())).unwrap();
}

#[test]
fn startup_fails_fast_on_missing_or_non_directory_root() {
    let tmp = tempfile::tempdir().unwrap();

    let missing = tmp.path().join("not-mounted");
    let err = agent::startup(&valid_env(&missing)).unwrap_err();
    assert!(
        matches!(&err, StartupError::RootMissing { path } if *path == missing),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("not-mounted"),
        "the message names the path: {err}"
    );

    let file = tmp.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    let err = agent::startup(&valid_env(&file)).unwrap_err();
    assert!(
        matches!(&err, StartupError::RootNotDirectory { path } if *path == file),
        "{err:?}"
    );
}

#[test]
fn a_bad_environment_stops_startup_before_the_root_is_touched() {
    // The root does not exist either, but the environment is checked first, so that is the error the operator sees.
    let tmp = tempfile::tempdir().unwrap();
    let mut env = valid_env(&tmp.path().join("not-mounted"));
    env.remove("LK_HUB_ENDPOINT");
    let err = agent::startup(&env).unwrap_err();
    assert!(
        matches!(
            err,
            StartupError::Settings(SettingsError::Missing("LK_HUB_ENDPOINT"))
        ),
        "{err:?}"
    );
}
