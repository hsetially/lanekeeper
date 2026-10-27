//! The hub refuses to start, says why, and leaks nothing (plan 18, T1; S7, S10, S21).
//!
//! Acceptance 3: with a setting missing or malformed, or a real component not there, the process stops with a typed message
//! that names the variable or the component, and the message holds no secret. The settings are refused before anything is
//! started (exit 2); anything that goes wrong while starting is exit 1.
//!
//! The first group runs the real `hub` binary and needs no Docker. The second runs the `hub_dev` example (the same start code with
//! files for secrets) against a real Postgres, because only a database that answers lets the start get as far as the web
//! certificate and the web root. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
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

use std::time::Duration;

use child::Child;
use rig::Rig;

const PLANTED: &str = "planted-secret-9137";

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// The settings of a serving hub, which the tests below take pieces away from. The database URL carries the planted password.
fn serving() -> (tempfile::TempDir, Vec<(&'static str, String)>) {
    let dir = tempfile::tempdir().unwrap();
    let url = dir.path().join("db-url");
    std::fs::write(&url, format!("postgres://hub:{PLANTED}@127.0.0.1:1/lanekeeper")).unwrap();
    let ca = dir.path().join("db-ca.pem");
    std::fs::write(&ca, "not a certificate").unwrap();
    let settings = vec![
        ("LK_DATABASE_URL_FILE", url.display().to_string()),
        ("LK_DB_CA_FILE", ca.display().to_string()),
        ("LK_WEB_CERT_REF", "web-cert".to_owned()),
        ("LK_WEB_KEY_REF", "web-key".to_owned()),
        ("LK_GCP_PROJECT", "lanekeeper-prod".to_owned()),
        ("LK_WEB_ADDR", "127.0.0.1:0".to_owned()),
        ("LK_OPS_ADDR", "127.0.0.1:0".to_owned()),
        ("LK_AGENT_ADDR", "127.0.0.1:0".to_owned()),
        ("LK_REPLICA_ADDR", "127.0.0.1:0".to_owned()),
        ("LK_START_TIMEOUT_SECS", "3".to_owned()),
        ("LK_AGENT_CA_KMS_REF", rig::AGENT_CA_KMS.to_owned()),
        ("LK_AGENT_CA_CERT_REF", "agent-ca-cert".to_owned()),
        ("LK_REPLICA_CA_KMS_REF", rig::REPLICA_CA_KMS.to_owned()),
        ("LK_REPLICA_CA_CERT_REF", "replica-ca-cert".to_owned()),
        ("LK_REPLICA_NAME", "hub-0".to_owned()),
        ("LK_REPLICA_ADDRESS", "hub-0.hub.lanekeeper.svc:8445".to_owned()),
        ("LK_HUB_AUDIENCE", "https://hub.lanekeeper.test".to_owned()),
        ("LK_AGENT_ENDPOINT_HOSTS", "agents.lanekeeper.test".to_owned()),
        (
            "LK_GIT_BASE_URL",
            "https://github.com/acme/configuration-base-saas.git".to_owned(),
        ),
        (
            "LK_GIT_TENANT_URL",
            "https://github.com/acme/csp-tenant-data.git".to_owned(),
        ),
        ("LK_GIT_BASE_REPO", "acme/configuration-base-saas".to_owned()),
        ("LK_GIT_TENANT_REPO", "acme/csp-tenant-data".to_owned()),
        // T5 (S8): the key that wraps the GitHub token vault.
        ("LK_VAULT_KMS_REF", rig::VAULT_KMS.to_owned()),
    ];
    let settings = settings.into_iter().chain(rig::identity_env()).collect();
    (dir, settings)
}

/// Run the real binary with these settings (minus `without`, plus `extra`) and return its exit code and everything it wrote.
fn run_hub(
    args: &[&str],
    without: &[&str],
    extra: &[(&str, &str)],
) -> (Option<i32>, String, tempfile::TempDir) {
    let (dir, mut settings) = serving();
    settings.retain(|(name, _)| !without.contains(name));
    let mut all: Vec<(String, String)> = settings.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    all.extend(env(extra));
    let mut hub = Child::spawn(&child::real_binary(), args, &all);
    let status = hub.exit_within(Duration::from_secs(20));
    (status.code(), hub.output(), dir)
}

fn assert_refused(code: Option<i32>, output: &str, want_code: i32, must_name: &str) {
    assert_eq!(code, Some(want_code), "{output}");
    assert!(
        output.contains(must_name),
        "the message does not name {must_name}: {output}"
    );
    assert!(
        !output.contains(PLANTED),
        "the message holds the planted secret: {output}"
    );
}

// ------------------------------------------------------------------------------------------------ T6: the MCP server

#[test]
fn both_mcp_authentication_modes_at_once_are_refused_by_name() {
    // D52: personal-token mode is never combined with OAuth, and a list that names both is a mistake and not a pick of one.
    for both in ["oauth,personal_token", "personal_token,oauth"] {
        let (code, output, _dir) = run_hub(&[], &[], &[("LK_MCP_AUTH", both)]);
        assert_refused(code, &output, 2, "LK_MCP_AUTH is not valid");
    }
}

#[test]
fn an_mcp_setting_that_is_wrong_is_refused_by_name_and_never_by_value() {
    for name in [
        "LK_MCP_AUTH",
        "LK_MCP_CONFIRM",
        "LK_MCP_CONTENT",
        "LK_MCP_NOTIFY_SERVICES",
        "LK_MCP_AS_MIRROR",
    ] {
        let (code, output, _dir) = run_hub(&[], &[], &[(name, PLANTED)]);
        assert_refused(code, &output, 2, &format!("{name} is not valid"));
    }
}

#[test]
fn oauth_mode_needs_an_application_id_uri_among_the_audiences_and_personal_token_mode_does_not() {
    // The scope a client asks for is `<application id URI>/mcp.access`: a list of the client id alone has nothing to build it from.
    let bare = "22222222-2222-4222-8222-222222222222";
    let (code, output, _dir) = run_hub(&[], &[], &[("LK_ENTRA_API_AUDIENCES", bare)]);
    assert_refused(code, &output, 2, "LK_ENTRA_API_AUDIENCES is not valid");
    // In personal-token mode the same list is fine: the settings are accepted, and the start goes on to fail where it always fails in
    // this test (the database is not there), which is exit 1 and not exit 2.
    let (code, output, _dir) = run_hub(
        &[],
        &[],
        &[
            ("LK_ENTRA_API_AUDIENCES", bare),
            ("LK_MCP_AUTH", "personal_token"),
        ],
    );
    assert_ne!(code, Some(2), "{output}");
}

// ------------------------------------------------------------------------------------------------ T5: the registry and the write paths

#[test]
fn without_the_vault_key() {
    let (code, output, _dir) = run_hub(&[], &["LK_VAULT_KMS_REF"], &[]);
    assert_refused(code, &output, 2, "LK_VAULT_KMS_REF is required");
}

#[test]
fn a_vault_key_that_is_a_version_or_not_a_key_is_refused_by_name() {
    // The vault wraps with the key and lets KMS pick the version, so a version is a mistake (a rotation would stop working).
    for bad in [
        "projects/p/locations/l/keyRings/r/cryptoKeys/vault/cryptoKeyVersions/1",
        "vault",
        "projects/p/locations/l/keyRings/r",
    ] {
        let (code, output, _dir) = run_hub(&[], &[], &[("LK_VAULT_KMS_REF", bad)]);
        assert_refused(code, &output, 2, "LK_VAULT_KMS_REF is not valid");
    }
}

#[test]
fn a_setting_of_the_registry_or_the_write_paths_that_is_wrong_is_refused_by_name_and_never_by_value() {
    for (name, bad) in [
        // GitHub is reached over HTTPS at one host: no scheme, a path, a port, a user or an upper-case name is a refused setting.
        ("LK_GITHUB_API_BASE", "http://api.github.com"),
        ("LK_GITHUB_API_BASE", "https://api.github.com/v3"),
        ("LK_GITHUB_API_BASE", "https://user@api.github.com"),
        ("LK_GIT_BASE_BRANCH", "ma in"),
        ("LK_CONFIG_SERVER_DEPLOYMENT", "Not A Deployment"),
        ("LK_TEAMS_ALLOWED_HOSTS", "*.example.com,"),
        ("LK_TEAMS_ALLOWED_HOSTS", "https://example.com"),
        // A switch that a typo could turn the wrong way is not a switch.
        ("LK_RETENTION_ENABLED", "yes"),
        ("LK_RETENTION_ENABLED", "TRUE"),
        ("LK_RETENTION_DRY_RUN", "0"),
        ("LK_EXTRA_DENY_GLOBS", "["),
        ("LK_EXTRA_DENY_GLOBS", "secrets/*,,x"),
        ("LK_CORPUS_CACHE_MIB", "1"),
        ("LK_WRITES_PR_JOBS_SECS", "0"),
        ("LK_WRITES_CLEANUP_SECS", "10"),
    ] {
        let (code, output, _dir) = run_hub(&[], &[], &[(name, bad)]);
        assert_refused(code, &output, 2, &format!("{name} is not valid"));
        assert!(
            !output.contains(bad),
            "{name}: the message holds the value: {output}"
        );
    }
}

#[test]
fn without_a_database() {
    let (code, output, _dir) = run_hub(&[], &["LK_DATABASE_URL_FILE"], &[]);
    assert_refused(code, &output, 2, "LK_DATABASE_URL_FILE is required");
}

#[test]
fn a_database_url_that_is_also_given_in_the_environment_is_refused_without_printing_either() {
    let (code, output, _dir) = run_hub(
        &[],
        &[],
        &[("LK_DATABASE_URL", &format!("postgres://x:{PLANTED}@h/d"))],
    );
    assert_refused(code, &output, 2, "LK_DATABASE_URL is not valid");
}

#[test]
fn without_the_web_certificate() {
    let (code, output, _dir) = run_hub(&[], &["LK_WEB_CERT_REF", "LK_WEB_KEY_REF"], &[]);
    assert_refused(code, &output, 2, "LK_WEB_CERT_REF is required");
}

#[test]
fn with_half_a_web_certificate() {
    let (code, output, _dir) = run_hub(&[], &["LK_WEB_KEY_REF"], &[]);
    assert_refused(code, &output, 2, "LK_WEB_KEY_REF is required");
    let (code, output, _dir) = run_hub(&[], &["LK_WEB_CERT_REF"], &[]);
    assert_refused(code, &output, 2, "LK_WEB_CERT_REF is required");
}

#[test]
fn without_the_google_project() {
    // Everything the settings need is there, so the hub gets as far as building its Google clients and says what is missing.
    let (code, output, _dir) = run_hub(&[], &["LK_GCP_PROJECT"], &[]);
    assert_refused(code, &output, 2, "LK_GCP_PROJECT is required");
}

// ------------------------------------------------------------------------------------------------ the platform (T3)

#[test]
fn without_the_agent_ca() {
    // All of the group gone: the group is absent, and serving names the first variable it needs.
    let (code, output, _dir) = run_hub(
        &[],
        &[
            "LK_AGENT_CA_KMS_REF",
            "LK_AGENT_CA_CERT_REF",
            "LK_REPLICA_CA_KMS_REF",
            "LK_REPLICA_CA_CERT_REF",
            "LK_REPLICA_NAME",
            "LK_REPLICA_ADDRESS",
            "LK_HUB_AUDIENCE",
            "LK_AGENT_ENDPOINT_HOSTS",
        ],
        &[],
    );
    assert_refused(code, &output, 2, "LK_AGENT_CA_KMS_REF is required");
    // Part of it gone: a half-written deployment is refused by the name of what is missing.
    for missing in [
        "LK_AGENT_CA_KMS_REF",
        "LK_AGENT_CA_CERT_REF",
        "LK_REPLICA_CA_KMS_REF",
        "LK_REPLICA_CA_CERT_REF",
    ] {
        let (code, output, _dir) = run_hub(&[], &[missing], &[]);
        assert_refused(code, &output, 2, &format!("{missing} is required"));
    }
}

#[test]
fn without_the_replica_name_or_address_or_audience_or_agent_hosts() {
    for missing in [
        "LK_REPLICA_NAME",
        "LK_REPLICA_ADDRESS",
        "LK_HUB_AUDIENCE",
        "LK_AGENT_ENDPOINT_HOSTS",
    ] {
        let (code, output, _dir) = run_hub(&[], &[missing], &[]);
        assert_refused(code, &output, 2, &format!("{missing} is required"));
    }
}

#[test]
fn without_the_git_remotes() {
    let (code, output, _dir) = run_hub(
        &[],
        &[
            "LK_GIT_BASE_URL",
            "LK_GIT_TENANT_URL",
            "LK_GIT_BASE_REPO",
            "LK_GIT_TENANT_REPO",
        ],
        &[],
    );
    assert_refused(code, &output, 2, "LK_GIT_BASE_URL is required");
    // One of four is half a deployment, named.
    let (code, output, _dir) = run_hub(&[], &["LK_GIT_TENANT_REPO"], &[]);
    assert_refused(code, &output, 2, "LK_GIT_TENANT_REPO is required");
}

// ------------------------------------------------------------------------------------------------ identity (T4)

/// The identity group of `serving()`, the names the tests below take away.
const IDENTITY_VARIABLES: [&str; 7] = [
    "LK_PUBLIC_ORIGIN",
    "LK_ENTRA_TENANT_ID",
    "LK_ENTRA_CLIENT_ID",
    "LK_ENTRA_CLIENT_AUTH_REF",
    "LK_ENTRA_COOKIE_KEY_REF",
    "LK_AUDIT_CHECKPOINT_BUCKET",
    "LK_AUDIT_CHECKPOINT_KMS_REF",
];

#[test]
fn without_the_entra_settings() {
    // All of the group gone: a hub that nobody can sign in to is not started, and the first variable it needs is named. There is
    // no session store of its own to fall back on (S1).
    let (code, output, _dir) = run_hub(&[], &IDENTITY_VARIABLES, &[]);
    assert_refused(code, &output, 2, "LK_ENTRA_TENANT_ID is required");
    // Part of it gone: a half-written deployment is refused by the name of what is missing.
    for missing in IDENTITY_VARIABLES {
        let (code, output, _dir) = run_hub(&[], &[missing], &[]);
        assert_refused(code, &output, 2, &format!("{missing} is required"));
    }
}

#[test]
fn a_malformed_identity_setting_names_the_variable_and_never_the_value() {
    for (name, value) in [
        ("LK_ENTRA_TENANT_ID", format!("{PLANTED}-not-a-guid")),
        ("LK_ENTRA_CLIENT_ID", "1234".to_owned()),
        // Plain HTTP, and a path: neither can be an origin.
        ("LK_PUBLIC_ORIGIN", format!("http://{PLANTED}.example")),
        ("LK_PUBLIC_ORIGIN", format!("https://{PLANTED}.example/app")),
        // A resource name where a secret's name belongs.
        (
            "LK_ENTRA_CLIENT_AUTH_REF",
            format!("projects/p/secrets/{PLANTED}"),
        ),
        ("LK_ENTRA_COOKIE_KEY_REF", format!("has space {PLANTED}")),
        ("LK_AUDIT_CHECKPOINT_BUCKET", format!("UPPER-{PLANTED}")),
        // A crypto key and not a version of one: a signature is made by one version.
        (
            "LK_AUDIT_CHECKPOINT_KMS_REF",
            format!("projects/p/locations/l/keyRings/r/cryptoKeys/{PLANTED}"),
        ),
        ("LK_BOOTSTRAP_ADMINS", format!("{PLANTED},not-a-guid")),
        ("LK_ENTRA_AUTHORITY_HOST", format!("UPPER.{PLANTED}")),
        ("LK_ENTRA_REDIRECT_PATH", format!("//{PLANTED}.example")),
    ] {
        let (code, output, _dir) = run_hub(&[], &[], &[(name, &value)]);
        assert_refused(code, &output, 2, &format!("{name} is not valid"));
    }
}

#[test]
fn a_malformed_setting_names_the_variable_and_never_the_value() {
    for (name, value) in [
        ("LK_WEB_ADDR", format!("{PLANTED}:not-a-port")),
        ("LK_START_TIMEOUT_SECS", PLANTED.to_owned()),
        ("LK_WEB_CERT_REF", format!("../{PLANTED}")),
        ("LK_GCP_PROJECT", format!("Not A Project {PLANTED}")),
        ("LK_TRUSTED_TYPES", format!("a; script-src * {PLANTED}")),
        ("LK_AGENT_CA_KMS_REF", format!("projects/{PLANTED}")),
        ("LK_REPLICA_CA_CERT_REF", format!("a/{PLANTED}")),
        ("LK_REPLICA_NAME", format!("pod/{PLANTED}")),
        ("LK_REPLICA_ADDRESS", format!("{PLANTED}:0")),
        ("LK_AGENT_ENDPOINT_HOSTS", format!("UPPER.{PLANTED}")),
        ("LK_AGENT_ADDR", format!("{PLANTED}:x")),
    ] {
        let (code, output, _dir) = run_hub(&[], &[], &[(name, &value)]);
        assert_refused(code, &output, 2, &format!("{name} is not valid"));
    }
}

#[test]
fn an_unknown_command_prints_the_usage_and_exits_2_without_repeating_it() {
    let (code, output, _dir) = run_hub(&[PLANTED], &[], &[]);
    assert_refused(code, &output, 2, "usage: hub [serve]");
    assert!(output.contains("unknown command"), "{output}");
    let (code, output, _dir) = run_hub(&["serve", PLANTED], &[], &[]);
    assert_refused(code, &output, 2, "unexpected argument");
}

#[test]
fn help_prints_the_usage_and_exits_0_without_starting_anything() {
    // No settings at all: help is not serving.
    let mut hub = Child::spawn(&child::real_binary(), &["help"], &[]);
    let status = hub.exit_within(Duration::from_secs(10));
    assert_eq!(status.code(), Some(0));
    assert!(hub.output().contains("usage: hub [serve]"));
}

#[test]
fn an_unreachable_database_is_exit_1_inside_the_start_timeout_and_the_message_names_the_database() {
    // Nothing listens on port 1. The hub has no database to fall back on, and it does not retry for ever.
    let (code, output, _dir) = run_hub(
        &[],
        &[],
        &[
            ("LK_START_TIMEOUT_SECS", "10"),
            ("LK_DB_ACQUIRE_TIMEOUT_SECS", "1"),
        ],
    );
    assert_refused(code, &output, 1, "the database is not usable");
    assert!(
        !output.contains("127.0.0.1"),
        "the host from the URL is in the message: {output}"
    );
    // Google is not reachable from the test either, and the start says so as well as the database, not instead of it: the web
    // certificate and the CA (whose certificate is a secret and whose key is in KMS) are each named.
    assert!(
        output.contains("the web certificate could not be loaded"),
        "{output}"
    );
    assert!(
        output.contains("the agent CA is not usable (LK_AGENT_CA_KMS_REF, LK_AGENT_CA_CERT_REF)"),
        "{output}"
    );
    assert!(
        !output.contains(rig::AGENT_CA_KMS),
        "the KMS key's name is in the output: {output}"
    );
}

// ------------------------------------------------------------------------------------------------ hub migrate (T2)

/// Run the real binary as `hub migrate` with exactly these variables.
fn run_migrate(args: &[&str], vars: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut hub = Child::spawn(&child::real_binary(), args, &env(vars));
    let status = hub.exit_within(Duration::from_secs(20));
    (status.code(), hub.output())
}

#[test]
fn migrate_names_what_is_missing_and_starts_nothing() {
    let (code, output) = run_migrate(&["migrate"], &[]);
    assert_refused(code, &output, 2, "LK_MIGRATOR_URL_FILE is required");
    let dir = tempfile::tempdir().unwrap();
    let url = dir.path().join("migrator-url");
    std::fs::write(
        &url,
        format!("postgres://lanekeeper_migrator:{PLANTED}@127.0.0.1:1/lanekeeper"),
    )
    .unwrap();
    let (code, output) = run_migrate(
        &["migrate"],
        &[("LK_MIGRATOR_URL_FILE", &url.display().to_string())],
    );
    assert_refused(code, &output, 2, "LK_DB_CA_FILE is required");
}

#[test]
fn the_hubs_own_database_credential_is_not_a_migrator_credential() {
    // A Job given the serving settings (the app role's URL file, its CA) is not a Job that can migrate: the migrator's URL is
    // its own variable, and the app role's variable is not read as a fallback.
    let (_dir, settings) = serving();
    let vars: Vec<(&str, &str)> = settings.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (code, output) = run_migrate(&["migrate"], &vars);
    assert_refused(code, &output, 2, "LK_MIGRATOR_URL_FILE is required");
}

#[test]
fn migrate_with_a_malformed_setting_names_the_variable_and_never_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let url = dir.path().join("migrator-url");
    std::fs::write(
        &url,
        format!("postgres://lanekeeper_migrator:{PLANTED}@127.0.0.1:1/x"),
    )
    .unwrap();
    let url = url.display().to_string();
    for (name, value) in [
        ("LK_MIGRATE_TIMEOUT_SECS", PLANTED.to_owned()),
        ("LK_DB_CA_FILE", format!("{PLANTED}\n")),
        ("LK_MIGRATOR_URL_FILE", format!("/nonexistent/{PLANTED}")),
    ] {
        let mut vars = vec![
            ("LK_MIGRATOR_URL_FILE", url.as_str()),
            ("LK_DB_CA_FILE", "/ca.pem"),
        ];
        vars.retain(|(n, _)| *n != name);
        vars.push((name, value.as_str()));
        let (code, output) = run_migrate(&["migrate"], &vars);
        assert_refused(code, &output, 2, &format!("{name} is not valid"));
    }
}

#[test]
fn migrate_against_a_database_that_is_not_there_is_exit_1_and_holds_no_secret() {
    // Nothing listens on port 1, and the CA file is not a certificate: the run ends, it does not hang, and says only that it
    // could not connect.
    let dir = tempfile::tempdir().unwrap();
    let url = dir.path().join("migrator-url");
    std::fs::write(
        &url,
        format!("postgres://lanekeeper_migrator:{PLANTED}@127.0.0.1:1/lanekeeper"),
    )
    .unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, "not a certificate").unwrap();
    let (code, output) = run_migrate(
        &["migrate"],
        &[
            ("LK_MIGRATOR_URL_FILE", &url.display().to_string()),
            ("LK_DB_CA_FILE", &ca.display().to_string()),
            ("LK_MIGRATE_TIMEOUT_SECS", "20"),
        ],
    );
    assert_refused(
        code,
        &output,
        1,
        "could not connect to the database as the migrator",
    );
    assert!(!output.contains("127.0.0.1"), "{output}");
}

#[test]
fn migrate_takes_no_arguments() {
    let (code, output) = run_migrate(&["migrate", PLANTED], &[]);
    assert_refused(code, &output, 2, "unexpected argument");
}

// ------------------------------------------------------------------------------------------------ hub ca-init (T3)

/// Run the real binary as `hub ca-init` with these arguments and no environment.
fn run_ca_init(args: &[&str]) -> (Option<i32>, String) {
    let mut all = vec!["ca-init"];
    all.extend_from_slice(args);
    let mut hub = Child::spawn(&child::real_binary(), &all, &[]);
    let status = hub.exit_within(Duration::from_secs(30));
    (status.code(), hub.output())
}

const KEY_VERSION: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/agent-ca/cryptoKeyVersions/1";

#[test]
fn ca_init_names_the_option_that_is_missing() {
    let (code, output) = run_ca_init(&[]);
    assert_refused(code, &output, 2, "--kms-ref is required");
    let (code, output) = run_ca_init(&["--kms-ref", KEY_VERSION]);
    assert_refused(code, &output, 2, "--common-name is required");
    let (code, output) = run_ca_init(&["--kms-ref", KEY_VERSION, "--common-name", "CA"]);
    assert_refused(code, &output, 2, "--valid-days is required");
}

#[test]
fn ca_init_never_repeats_a_value_it_refuses() {
    let secret = format!("{PLANTED}/x");
    for (option, args) in [
        (
            "--kms-ref",
            vec![
                "--kms-ref",
                secret.as_str(),
                "--common-name",
                "CA",
                "--valid-days",
                "30",
            ],
        ),
        (
            "--common-name",
            vec![
                "--kms-ref",
                KEY_VERSION,
                "--common-name",
                secret.as_str(),
                "--valid-days",
                "30",
            ],
        ),
        (
            "--valid-days",
            vec![
                "--kms-ref",
                KEY_VERSION,
                "--common-name",
                "CA",
                "--valid-days",
                secret.as_str(),
            ],
        ),
    ] {
        let (code, output) = run_ca_init(&args);
        assert_refused(code, &output, 2, &format!("{option} is not valid"));
    }
    // An argument it does not know is not echoed either.
    let (code, output) = run_ca_init(&[PLANTED]);
    assert_refused(code, &output, 2, "unexpected argument");
}

#[test]
fn ca_init_without_google_is_exit_1_names_the_step_and_prints_no_certificate() {
    // Valid options and no KMS to reach: the command fails whole, and what it prints is a message and no PEM.
    let (code, output) = run_ca_init(&[
        "--kms-ref",
        KEY_VERSION,
        "--common-name",
        "CA",
        "--valid-days",
        "30",
    ]);
    assert_refused(
        code,
        &output,
        1,
        "the public key of the KMS key could not be read",
    );
    assert!(!output.contains("BEGIN CERTIFICATE"), "{output}");
    assert!(
        !output.contains(KEY_VERSION),
        "the key's name is in the message: {output}"
    );
}

// ------------------------------------------------------------------------------------------------ with a real database

#[tokio::test]
async fn a_missing_certificate_secret_is_exit_1_and_names_the_settings() {
    let Some(rig) = Rig::start().await else { return };
    std::fs::remove_file(rig.secrets.path().join("web-key")).unwrap();
    let mut hub = rig.spawn(&[]).await;
    let status = hub.exit_within(Duration::from_secs(30));
    assert_eq!(status.code(), Some(1), "{}", hub.output());
    let output = hub.output();
    assert!(
        output.contains("LK_WEB_CERT_REF") && output.contains("the key secret could not be read"),
        "{output}"
    );
    for secret in [tls_pg::PLANTED_PASSWORD, rig.leaf.key_pem.lines().nth(1).unwrap()] {
        assert!(!output.contains(secret), "{output}");
    }
}

#[tokio::test]
async fn a_web_root_without_the_nonce_placeholder_is_exit_1_and_names_the_setting() {
    let Some(rig) = Rig::start().await else { return };
    // A build that forgot the placeholder would serve a page whose scripts the CSP blocks: a blank page, not an error.
    std::fs::write(
        rig.dist.path().join("index.html"),
        "<html><body>no placeholder</body></html>",
    )
    .unwrap();
    let mut hub = rig.spawn(&[]).await;
    let status = hub.exit_within(Duration::from_secs(30));
    assert_eq!(status.code(), Some(1), "{}", hub.output());
    assert!(hub.output().contains("LK_WEB_DIST"), "{}", hub.output());
}

#[tokio::test]
async fn an_address_that_is_taken_is_exit_1_and_names_the_listener() {
    let Some(rig) = Rig::start().await else { return };
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    let mut hub = rig.spawn(&[("LK_WEB_ADDR", &address)]).await;
    let status = hub.exit_within(Duration::from_secs(30));
    assert_eq!(status.code(), Some(1), "{}", hub.output());
    assert!(
        hub.output().contains("could not listen on the web address"),
        "{}",
        hub.output()
    );
    drop(taken);
}

#[tokio::test]
async fn an_unmigrated_database_is_exit_1_names_the_migration_and_is_left_as_it_was() {
    // The roles exist and the schema does not: nobody has run `hub migrate`. The hub holds no credential that could, so it
    // says which migration is missing and stops (G4, G22). It does not start and fall over on its first query.
    let Some(rig) = Rig::start_unmigrated().await else {
        return;
    };
    let mut hub = rig.spawn(&[]).await;
    let status = hub.exit_within(Duration::from_secs(30));
    assert_eq!(status.code(), Some(1), "{}", hub.output());
    let output = hub.output();
    assert!(
        output.contains("the database schema is not usable")
            && output.contains("migration 0001 (init)")
            && output.contains("hub migrate"),
        "{output}"
    );
    for secret in [tls_pg::APP_ROLE.1, tls_pg::PLANTED_PASSWORD] {
        assert!(!output.contains(secret), "{output}");
    }
    // And it changed nothing: still no schema.
    let pool = rig.pg.app_pool().await;
    assert_eq!(
        hub::schema::missing(&pool).await.unwrap().len(),
        hub::migrate::EMBEDDED.len()
    );
}
