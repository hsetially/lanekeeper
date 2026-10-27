//! The committed fuzz seeds, replayed on stable (plan 18, T1 and T2; rule 13; S7, S10).
//!
//! `crates/hub/fuzz/` holds the fuzz targets (nightly, `just hub-fuzz`) and `invariants.rs`, which this test includes: every
//! seed in `fuzz/corpus/<target>/` is replayed with the checks the fuzzer runs, so `cargo test -p hub` (and so `just verify`)
//! catches a regression in a parser at a trust boundary without a nightly toolchain. A seed's name says what it must still do:
//! `accepted_*` is accepted, `rejected_*` refused; a test fails if a seed stops meaning that.
//!
//! The seeds hold no secret: the key bodies are garbage between real PEM framing, and the certificate is a public throwaway.
//! `UPDATE_HUB_SEEDS=1 cargo test -p hub --test corpus_replay regenerate_the_seeds` rewrites them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::too_many_lines
)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use invariants::{Check, Verdict};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn corpus(target: &str) -> PathBuf {
    fuzz_dir().join("corpus").join(target)
}

fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = corpus(target);
    let mut out: Vec<(String, Vec<u8>)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn replay(target: &str, check: Check) {
    let all = seeds(target);
    assert!(all.len() >= 5, "{target} has only {} seeds", all.len());
    for (name, data) in &all {
        let verdict = check(data).unwrap_or_else(|why| panic!("{target}/{name}: {why}"));
        let want = if name.starts_with("accepted_") {
            Verdict::Accepted
        } else if name.starts_with("rejected_") {
            Verdict::Rejected
        } else {
            panic!("{target}/{name}: a seed's name must start with accepted_ or rejected_");
        };
        assert_eq!(
            verdict, want,
            "{target}/{name} no longer means what its name says"
        );
    }
}

#[test]
fn every_settings_seed_keeps_its_verdict_and_its_invariants() {
    replay("hub_settings", invariants::hub_settings);
}

#[test]
fn every_command_line_seed_keeps_its_verdict_and_its_invariants() {
    replay("hub_cli_args", invariants::hub_cli_args);
}

#[test]
fn the_migrator_seeds_keep_the_migrators_verdict() {
    // `hub migrate` reads the same shape of input as the hub (T2). A seed's name gives the serving hub's verdict; these three
    // exist for the migrator's, and a test fails if one stops meaning what it says.
    for (name, want) in [
        ("accepted_migrator_job", Verdict::Accepted),
        ("accepted_migrator_job_with_a_bad_timeout", Verdict::Rejected),
        ("accepted_migrator_job_without_its_credential", Verdict::Rejected),
    ] {
        let data = fs::read(corpus("hub_settings").join(name)).unwrap();
        assert_eq!(invariants::migrator_settings(&data), Ok(want), "{name}");
        // The serving hub accepts all three: it reads none of the migrator's variables.
        assert_eq!(invariants::hub_settings(&data), Ok(Verdict::Accepted), "{name}");
    }
}

#[test]
fn every_pem_seed_keeps_its_verdict_and_its_invariants() {
    replay("hub_pem", invariants::hub_pem);
}

#[test]
fn the_ca_certificate_seeds_keep_the_ca_decoders_verdict() {
    // The first half of every `hub_pem` seed is also read as a CA certificate secret (T3). The seeds below are the ones whose
    // verdict there differs from the web decoder's, or that exist for it; a test fails if one stops meaning what it says.
    for (name, want) in [
        // One certificate and no key: a fine CA certificate secret, and no web certificate.
        ("rejected_certificate_without_a_key", Verdict::Accepted),
        ("rejected_ca_two_certificates", Verdict::Rejected),
        ("rejected_ca_certificate_beside_a_private_key", Verdict::Rejected),
        ("rejected_nine_certificates", Verdict::Rejected),
        ("rejected_chain_with_a_garbage_certificate", Verdict::Rejected),
        ("rejected_bad_base64_in_the_chain", Verdict::Rejected),
        ("rejected_not_pem", Verdict::Rejected),
        ("rejected_empty", Verdict::Rejected),
    ] {
        let data = fs::read(corpus("hub_pem").join(name)).unwrap();
        assert_eq!(invariants::ca_certificate(&data), Ok(want), "{name}");
        // And the web decoder refuses all of them, as the names say.
        assert_eq!(invariants::hub_pem(&data), Ok(Verdict::Rejected), "{name}");
    }
}

#[test]
fn the_platform_seeds_mean_what_their_names_say() {
    // `accepted_platform` is a complete platform group; the half of one, a replica address in upper case (which would be refused
    // only when the CA's profile saw it), a wildcard host and a KMS reference that names a key and not a version are each refused
    // by name. `every_settings_seed_keeps_its_verdict...` holds the verdicts; this holds that the platform group is really read.
    let read = |name: &str| fs::read(corpus("hub_settings").join(name)).unwrap();
    let parse = |data: &[u8]| {
        let text = String::from_utf8_lossy(data).into_owned();
        let vars: std::collections::BTreeMap<String, String> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        hub::HubSettings::from_lookup(move |name| vars.get(name).cloned())
    };
    let platform = parse(&read("accepted_platform")).unwrap().platform.unwrap();
    assert_eq!(platform.agent_hosts.len(), 2);
    for (name, variable) in [
        ("rejected_platform_half", "LK_AGENT_CA_KMS_REF"),
        ("rejected_replica_address_in_upper_case", "LK_REPLICA_ADDRESS"),
        ("rejected_agent_hosts_with_a_wildcard", "LK_AGENT_ENDPOINT_HOSTS"),
        ("rejected_kms_reference_that_is_a_key", "LK_AGENT_CA_KMS_REF"),
    ] {
        let error = parse(&read(name)).unwrap_err().to_string();
        assert!(error.starts_with(variable), "{name}: {error}");
    }
}

#[test]
fn the_identity_seeds_mean_what_their_names_say() {
    // `accepted_identity` is a complete identity group; a missing client id, an origin over plain HTTP, with a path or with userinfo,
    // the same first admin twice, a checkpoint key that is a key and not a version, and a secret reference that is a path are each
    // refused by name. `every_settings_seed_keeps_its_verdict...` holds the verdicts; this holds that the group is really read.
    let read = |name: &str| fs::read(corpus("hub_settings").join(name)).unwrap();
    let parse = |data: &[u8]| {
        let text = String::from_utf8_lossy(data).into_owned();
        let vars: std::collections::BTreeMap<String, String> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        hub::HubSettings::from_lookup(move |name| vars.get(name).cloned())
    };
    let identity = parse(&read("accepted_identity")).unwrap().identity.unwrap();
    assert_eq!(identity.public_origin(), "https://hub.lanekeeper.example");
    let more = parse(&read("accepted_identity_with_the_first_admins_and_an_older_key"))
        .unwrap()
        .identity
        .unwrap();
    assert_eq!(more.config.bootstrap_admins.len(), 2);
    assert_eq!(more.checkpoint.older_keys.len(), 1);
    for (name, variable) in [
        ("rejected_identity_half", "LK_ENTRA_CLIENT_ID"),
        ("rejected_identity_origin_over_plain_http", "LK_PUBLIC_ORIGIN"),
        ("rejected_identity_origin_with_a_path", "LK_PUBLIC_ORIGIN"),
        ("rejected_identity_origin_with_userinfo", "LK_PUBLIC_ORIGIN"),
        ("rejected_identity_first_admins_twice", "LK_BOOTSTRAP_ADMINS"),
        (
            "rejected_identity_checkpoint_key_that_is_a_key",
            "LK_AUDIT_CHECKPOINT_KMS_REF",
        ),
        (
            "rejected_identity_secret_name_that_is_a_path",
            "LK_ENTRA_CLIENT_AUTH_REF",
        ),
    ] {
        let error = parse(&read(name)).unwrap_err().to_string();
        assert!(error.starts_with(variable), "{name}: {error}");
    }
}

#[test]
fn the_registry_and_write_seeds_mean_what_their_names_say() {
    // The group is really read: the accepted seeds carry their values, and each rejected seed is refused by the variable it is about.
    let read = |name: &str| fs::read(corpus("hub_settings").join(name)).unwrap();
    let parse = |data: &[u8]| {
        let text = String::from_utf8_lossy(data).into_owned();
        let vars: std::collections::BTreeMap<String, String> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        hub::HubSettings::from_lookup(move |name| vars.get(name).cloned())
    };
    let defaults = parse(&read("accepted_writes_with_the_defaults")).unwrap();
    assert!(!defaults.registry.retention_enabled && defaults.registry.config.retention_dry_run);
    assert_eq!(defaults.writes.unwrap().github_api_host, "api.github.com");
    let all = parse(&read("accepted_registry_and_writes")).unwrap();
    assert!(all.registry.retention_enabled && !all.registry.config.retention_dry_run);
    assert_eq!(all.registry.config.extra_deny_globs, ["secrets/**", "**/*.pem"]);
    assert_eq!(all.writes.unwrap().github_api_host, "ghe.example.com");
    for (name, variable) in [
        ("rejected_vault_key_that_is_a_version", "LK_VAULT_KMS_REF"),
        ("rejected_retention_switch_with_a_typo", "LK_RETENTION_ENABLED"),
        ("rejected_dry_run_switch_that_is_a_number", "LK_RETENTION_DRY_RUN"),
        ("rejected_deny_glob_that_does_not_compile", "LK_EXTRA_DENY_GLOBS"),
        ("rejected_deny_globs_with_an_empty_entry", "LK_EXTRA_DENY_GLOBS"),
        ("rejected_corpus_cache_below_its_floor", "LK_CORPUS_CACHE_MIB"),
        ("rejected_github_base_over_plain_http", "LK_GITHUB_API_BASE"),
        ("rejected_github_base_with_a_port", "LK_GITHUB_API_BASE"),
        ("rejected_teams_host_that_is_a_url", "LK_TEAMS_ALLOWED_HOSTS"),
        ("rejected_writes_job_interval_zero", "LK_WRITES_PR_JOBS_SECS"),
    ] {
        let error = parse(&read(name)).unwrap_err().to_string();
        assert!(error.starts_with(variable), "{name}: {error}");
    }
}

#[test]
fn the_mcp_seeds_mean_what_their_names_say() {
    // The switches are really read: the accepted seeds carry their values, each rejected seed is refused by the variable it is about,
    // and the audience that OAuth builds its scope from is the application id URI.
    let read = |name: &str| fs::read(corpus("hub_settings").join(name)).unwrap();
    let parse = |data: &[u8]| {
        let text = String::from_utf8_lossy(data).into_owned();
        let vars: std::collections::BTreeMap<String, String> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        hub::HubSettings::from_lookup(move |name| vars.get(name).cloned())
    };
    let all = parse(&read("accepted_mcp_every_switch_set")).unwrap();
    assert_eq!(all.mcp.config.auth_mode.to_string(), "personal_token");
    assert_eq!(all.mcp.config.confirm_mode.to_string(), "draft");
    assert_eq!(all.mcp.config.content_mode.to_string(), "data_block");
    assert!(all.mcp.notify_services && all.mcp.authorization_server_mirror);
    let oauth = parse(&read("accepted_identity")).unwrap();
    assert_eq!(oauth.mcp, hub::config::McpSettings::default());
    let built = oauth.mcp.for_identity(oauth.identity.as_ref().unwrap()).unwrap();
    assert_eq!(
        built.oauth.unwrap().api_audience,
        "api://22222222-2222-4222-8222-222222222222"
    );
    assert!(parse(&read("accepted_mcp_personal_token_mode_with_a_bare_audience")).is_ok());
    for (name, variable) in [
        ("rejected_mcp_both_authentication_modes", "LK_MCP_AUTH"),
        ("rejected_mcp_authentication_list_that_is_too_long", "LK_MCP_AUTH"),
        ("rejected_mcp_confirm_mode_that_is_not_one", "LK_MCP_CONFIRM"),
        ("rejected_mcp_content_mode_in_the_wrong_case", "LK_MCP_CONTENT"),
        (
            "rejected_mcp_notify_switch_that_is_a_word",
            "LK_MCP_NOTIFY_SERVICES",
        ),
        (
            "rejected_mcp_oauth_without_an_application_id_uri",
            "LK_ENTRA_API_AUDIENCES",
        ),
    ] {
        let error = parse(&read(name)).unwrap_err().to_string();
        assert!(error.starts_with(variable), "{name}: {error}");
    }
}

#[test]
fn the_corpus_has_a_directory_for_every_target_and_nothing_else() {
    let targets: BTreeSet<String> = fs::read_dir(fuzz_dir().join("fuzz_targets"))
        .unwrap()
        .filter_map(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .strip_suffix(".rs")
                .map(str::to_owned)
        })
        .collect();
    let dirs: BTreeSet<String> = fs::read_dir(fuzz_dir().join("corpus"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(targets, dirs);
    assert_eq!(
        targets,
        ["hub_cli_args", "hub_pem", "hub_settings"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    );
}

/// The accepted path of the PEM parser has no committed seed (that would be a private key in the repository), so a pair made
/// here stands in: a chain and its key are accepted, and the same chain with another key is refused.
#[test]
fn a_fresh_valid_pair_is_accepted_and_a_mismatched_one_is_not() {
    let make = || {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["hub.example".to_owned()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        (cert.pem(), key.serialize_pem())
    };
    let ((chain, key), (_, other_key)) = (make(), make());
    let join = |chain: &str, key: &str| [chain.as_bytes(), &[0], key.as_bytes()].concat();
    assert_eq!(invariants::hub_pem(&join(&chain, &key)), Ok(Verdict::Accepted));
    assert_eq!(
        invariants::hub_pem(&join(&chain, &other_key)),
        Ok(Verdict::Rejected)
    );
}

#[test]
fn the_invariants_notice_a_settings_error_that_repeats_a_value() {
    // The check that guards the error text is itself checked against messages that break it.
    for fine in [
        "LK_DB_MAX_CONNECTIONS is not valid",
        "LK_WEB_CERT_REF is required",
        "LK_A1 is required",
    ] {
        assert!(invariants::is_fixed_settings_error(fine), "{fine}");
    }
    for bad in [
        "LK_DB_MAX_CONNECTIONS is not valid: lots",
        "LK_DB_MAX_CONNECTIONS is not valid (lots)",
        "lots is not valid",
        "LK_x is required",
        "LK_WEB_ADDR=127.0.0.1:1 is not valid",
        "",
    ] {
        assert!(!invariants::is_fixed_settings_error(bad), "{bad}");
    }
}

/// A key version for the `ca-init` seeds.
const KEY_VERSION: &str =
    "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/agent-ca/cryptoKeyVersions/1";

/// The key that wraps the token vault (T5): a crypto key, not a version.
const VAULT_KEY: &str = "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/vault";

/// The platform group, complete (T3).
/// The identity group (T4), complete. The identity seeds are this with one line changed, added or dropped.
const IDENTITY: &str = "LK_ENTRA_TENANT_ID=11111111-1111-4111-8111-111111111111\nLK_ENTRA_CLIENT_ID=22222222-2222-4222-8222-222222222222\nLK_PUBLIC_ORIGIN=https://hub.lanekeeper.example\nLK_ENTRA_CLIENT_AUTH_REF=lanekeeper-entra-client\nLK_ENTRA_COOKIE_KEY_REF=lanekeeper-login-cookie\nLK_AUDIT_CHECKPOINT_BUCKET=lanekeeper-audit-checkpoints\nLK_AUDIT_CHECKPOINT_KMS_REF=projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/audit/cryptoKeyVersions/1\n";

const PLATFORM: &str = "LK_AGENT_CA_KMS_REF=projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/agent-ca/cryptoKeyVersions/1\nLK_AGENT_CA_CERT_REF=lanekeeper-agent-ca\nLK_REPLICA_CA_KMS_REF=projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/replica-ca/cryptoKeyVersions/1\nLK_REPLICA_CA_CERT_REF=lanekeeper-replica-ca\nLK_REPLICA_NAME=lanekeeper-hub-0\nLK_REPLICA_ADDRESS=lanekeeper-hub-0.lanekeeper-hub.lanekeeper.svc:8445\nLK_HUB_AUDIENCE=https://hub.lanekeeper.example\nLK_AGENT_ENDPOINT_HOSTS=agents.lanekeeper.example,agents-2.lanekeeper.example\n";

/// `ca-init` followed by `args`, as the NUL-separated input of the `hub_cli_args` target.
fn ca_init(args: &[&str]) -> String {
    let mut all = vec!["ca-init"];
    all.extend_from_slice(args);
    all.join("\0")
}

fn write_seeds(target: &str, seeds: &[(&str, Vec<u8>)]) {
    let dir = corpus(target);
    fs::create_dir_all(&dir).unwrap();
    for entry in fs::read_dir(&dir).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for (name, data) in seeds {
        fs::write(dir.join(name), data).unwrap();
    }
}

/// Rewrites the committed seeds. Not a check: it runs only when asked.
#[test]
fn regenerate_the_seeds() {
    if std::env::var_os("UPDATE_HUB_SEEDS").is_none() {
        return;
    }
    let text = |s: &str| s.as_bytes().to_vec();
    write_seeds(
        "hub_settings",
        &[
            ("accepted_empty", text("")),
            (
                "accepted_serving",
                text(
                    "LK_DATABASE_URL=postgres://u:p@db/x\nLK_DB_CA_FILE=/ca.pem\nLK_WEB_CERT_REF=web-cert\nLK_WEB_KEY_REF=web-key\nLK_GCP_PROJECT=lanekeeper-prod\n",
                ),
            ),
            (
                "accepted_database_url_from_a_file",
                text(
                    "LK_DATABASE_URL_FILE=/run/db/uri\nLK_DB_CA_FILE=/run/ca/ca.pem\nFILE_CONTENT=postgres://u:hunter2@db.internal/x\n",
                ),
            ),
            (
                "accepted_git_remotes",
                text(
                    "LK_GIT_BASE_URL=https://github.com/acme/configuration-base-saas.git\nLK_GIT_TENANT_URL=https://github.com/acme/csp-tenant-data.git\nLK_GIT_BASE_REPO=acme/configuration-base-saas\nLK_GIT_TENANT_REPO=acme/csp-tenant-data\nLK_GIT_READ_REF=my-read-token\nLK_GIT_WEBHOOK_REF=my-hook\n",
                ),
            ),
            (
                "accepted_every_number_at_its_edge",
                text(
                    "LK_START_TIMEOUT_SECS=120\nLK_WEB_CERT_RELOAD_SECS=86400\nLK_SHUTDOWN_GRACE_SECS=0\nLK_BODY_MAX_BYTES=67108864\nLK_TRUSTED_PROXY_HOPS=8\n",
                ),
            ),
            // T5: the registry's knobs and the write paths' group.
            (
                "accepted_writes_with_the_defaults",
                text(&format!("LK_VAULT_KMS_REF={VAULT_KEY}\n")),
            ),
            (
                "accepted_registry_and_writes",
                text(&format!(
                    "LK_VAULT_KMS_REF={VAULT_KEY}\nLK_EXTRA_DENY_GLOBS=secrets/**, **/*.pem\nLK_RETENTION_ENABLED=true\nLK_RETENTION_DRY_RUN=false\nLK_CORPUS_CACHE_MIB=64\nLK_GITHUB_API_BASE=https://ghe.example.com/\nLK_GIT_BASE_BRANCH=release/base\nLK_CONFIG_SERVER_DEPLOYMENT=config-server\nLK_TEAMS_ALLOWED_HOSTS=*.logic.azure.com,hooks.example.com\nLK_WRITES_RECONCILE_SECS=1\nLK_WRITES_EXPIRE_SECS=86400\nLK_WRITES_PR_JOBS_SECS=300\nLK_WRITES_CLEANUP_SECS=60\n"
                )),
            ),
            (
                "rejected_vault_key_that_is_a_version",
                text(&format!("LK_VAULT_KMS_REF={VAULT_KEY}/cryptoKeyVersions/1\n")),
            ),
            (
                "rejected_retention_switch_with_a_typo",
                text("LK_RETENTION_ENABLED=yes\n"),
            ),
            (
                "rejected_dry_run_switch_that_is_a_number",
                text("LK_RETENTION_DRY_RUN=0\n"),
            ),
            (
                "rejected_deny_glob_that_does_not_compile",
                text("LK_EXTRA_DENY_GLOBS=[\n"),
            ),
            (
                "rejected_deny_globs_with_an_empty_entry",
                text("LK_EXTRA_DENY_GLOBS=secrets/**,,x\n"),
            ),
            (
                "rejected_corpus_cache_below_its_floor",
                text("LK_CORPUS_CACHE_MIB=8\n"),
            ),
            (
                "rejected_github_base_over_plain_http",
                text("LK_GITHUB_API_BASE=http://api.github.com\n"),
            ),
            (
                "rejected_github_base_with_a_port",
                text("LK_GITHUB_API_BASE=https://api.github.com:8443\n"),
            ),
            (
                "rejected_teams_host_that_is_a_url",
                text("LK_TEAMS_ALLOWED_HOSTS=https://hooks.example.com\n"),
            ),
            (
                "rejected_writes_job_interval_zero",
                text("LK_WRITES_PR_JOBS_SECS=0\n"),
            ),
            // The MCP server's switches (T6, G12) and how they fit the Entra settings.
            (
                "accepted_mcp_every_switch_set",
                text(&format!(
                    "{IDENTITY}LK_MCP_AUTH=personal_token\nLK_MCP_CONFIRM=draft\nLK_MCP_CONTENT=data_block\nLK_MCP_NOTIFY_SERVICES=true\nLK_MCP_AS_MIRROR=true\n"
                )),
            ),
            (
                "accepted_mcp_personal_token_mode_with_a_bare_audience",
                text(&format!(
                    "{IDENTITY}LK_MCP_AUTH=personal_token\nLK_ENTRA_API_AUDIENCES=22222222-2222-4222-8222-222222222222\n"
                )),
            ),
            (
                "rejected_mcp_both_authentication_modes",
                text("LK_MCP_AUTH=oauth,personal_token\n"),
            ),
            (
                "rejected_mcp_authentication_list_that_is_too_long",
                text("LK_MCP_AUTH=oauth,oauth,oauth,oauth,oauth\n"),
            ),
            (
                "rejected_mcp_confirm_mode_that_is_not_one",
                text("LK_MCP_CONFIRM=ask\n"),
            ),
            (
                "rejected_mcp_content_mode_in_the_wrong_case",
                text("LK_MCP_CONTENT=Summary\n"),
            ),
            (
                "rejected_mcp_notify_switch_that_is_a_word",
                text("LK_MCP_NOTIFY_SERVICES=yes\n"),
            ),
            (
                "rejected_mcp_oauth_without_an_application_id_uri",
                text(&format!(
                    "{IDENTITY}LK_ENTRA_API_AUDIENCES=22222222-2222-4222-8222-222222222222\n"
                )),
            ),
            ("rejected_half_a_certificate", text("LK_WEB_CERT_REF=web-cert\n")),
            (
                "rejected_project_with_a_space",
                text("LK_GCP_PROJECT=Has Space\n"),
            ),
            ("rejected_timeout_zero", text("LK_START_TIMEOUT_SECS=0\n")),
            (
                "rejected_control_character",
                text("LK_WEB_ADDR=127.0.0.1:1\u{7}\n"),
            ),
            (
                "rejected_a_bad_number_beside_a_password",
                text(
                    "LK_DATABASE_URL=postgres://u:hunter2@db/x\nLK_DB_CA_FILE=/ca\nLK_DB_MAX_CONNECTIONS=lots\n",
                ),
            ),
            (
                "rejected_both_database_forms",
                text(
                    "LK_DATABASE_URL_FILE=/u\nLK_DATABASE_URL=postgres://x/y\nLK_DB_CA_FILE=/ca\nFILE_CONTENT=postgres://a/b\n",
                ),
            ),
            (
                "rejected_a_secret_name_with_a_slash",
                text("LK_WEB_CERT_REF=a/b\nLK_WEB_KEY_REF=k\n"),
            ),
            (
                "rejected_csp_injection",
                text("LK_TRUSTED_TYPES=a; script-src *\n"),
            ),
            // The platform group (T3, G10): the CAs, this replica's identity and the agent endpoint.
            ("accepted_platform", text(PLATFORM)),
            (
                "rejected_platform_half",
                text("LK_REPLICA_NAME=lanekeeper-hub-0\n"),
            ),
            (
                "rejected_replica_address_in_upper_case",
                text(&PLATFORM.replace(
                    "lanekeeper-hub-0.lanekeeper-hub",
                    "Lanekeeper-Hub-0.lanekeeper-hub",
                )),
            ),
            (
                "rejected_agent_hosts_with_a_wildcard",
                text(&PLATFORM.replace("agents.lanekeeper.example", "*.lanekeeper.example")),
            ),
            (
                "rejected_kms_reference_that_is_a_key",
                text(&PLATFORM.replacen("/cryptoKeyVersions/1", "", 1)),
            ),
            // `hub migrate`'s settings (T2): the serving hub accepts all three, because it reads none of these variables.
            (
                "accepted_migrator_job",
                text(
                    "LK_MIGRATOR_URL_FILE=/run/migrator/uri\nLK_DB_CA_FILE=/run/db-ca/ca.crt\nLK_MIGRATE_TIMEOUT_SECS=900\nFILE_CONTENT=postgres://lanekeeper_migrator:not-a-real-password@db.example/lanekeeper\n",
                ),
            ),
            (
                "accepted_migrator_job_with_a_bad_timeout",
                text(
                    "LK_MIGRATOR_URL_FILE=/run/migrator/uri\nLK_DB_CA_FILE=/run/db-ca/ca.crt\nLK_MIGRATE_TIMEOUT_SECS=0\nFILE_CONTENT=postgres://lanekeeper_migrator:not-a-real-password@db.example/lanekeeper\n",
                ),
            ),
            (
                "accepted_migrator_job_without_its_credential",
                text("LK_DB_CA_FILE=/run/db-ca/ca.crt\nLK_MIGRATE_TIMEOUT_SECS=900\n"),
            ),
            // The identity group (T4). They were committed by hand and the generator deleted them; they are generated now.
            ("accepted_identity", text(IDENTITY)),
            (
                "accepted_identity_with_the_first_admins_and_an_older_key",
                text(&format!(
                    "{IDENTITY}LK_BOOTSTRAP_ADMINS=33333333-3333-4333-8333-333333333333,44444444-4444-4444-8444-444444444444\nLK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS=projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/audit/cryptoKeyVersions/0\nLK_ENTRA_API_AUDIENCES=api://lanekeeper,22222222-2222-4222-8222-222222222222\n"
                )),
            ),
            (
                "rejected_identity_checkpoint_key_that_is_a_key",
                text(&IDENTITY.replacen("/cryptoKeyVersions/1", "", 1)),
            ),
            (
                "rejected_identity_first_admins_twice",
                text(&format!(
                    "{IDENTITY}LK_BOOTSTRAP_ADMINS=33333333-3333-4333-8333-333333333333,33333333-3333-4333-8333-333333333333\n"
                )),
            ),
            (
                "rejected_identity_half",
                text(&IDENTITY.replacen("LK_ENTRA_CLIENT_ID=22222222-2222-4222-8222-222222222222\n", "", 1)),
            ),
            (
                "rejected_identity_origin_over_plain_http",
                text(&IDENTITY.replacen("https://hub", "http://hub", 1)),
            ),
            (
                "rejected_identity_origin_with_a_path",
                text(&IDENTITY.replacen("hub.lanekeeper.example\n", "hub.lanekeeper.example/app\n", 1)),
            ),
            (
                "rejected_identity_origin_with_userinfo",
                text(&IDENTITY.replacen("https://hub", "https://admin@hub", 1)),
            ),
            (
                "rejected_identity_secret_name_that_is_a_path",
                text(&IDENTITY.replacen("lanekeeper-entra-client", "projects/p/secrets/s", 1)),
            ),
        ],
    );
    write_seeds(
        "hub_cli_args",
        &[
            ("accepted_no_arguments", text("")),
            ("accepted_serve", text("serve")),
            ("accepted_help", text("help")),
            ("accepted_long_help", text("--help")),
            ("accepted_short_help", text("-h")),
            ("accepted_migrate", text("migrate")),
            ("rejected_unknown_command", text("restart")),
            ("rejected_an_empty_word", vec![0]),
            ("rejected_extra_argument", text("serve\0now")),
            ("rejected_a_pasted_secret", text("hunter2-planted-secret")),
            ("rejected_help_then_serve", text("help\0serve")),
            ("rejected_migrate_extra_argument", text("migrate\0now")),
            ("rejected_migrate_capitalised", text("Migrate")),
            ("rejected_help_then_migrate", text("help\0migrate")),
            // `ca-init` and its three options (T3, G15).
            (
                "accepted_ca_init",
                text(&ca_init(&[
                    "--kms-ref",
                    KEY_VERSION,
                    "--common-name",
                    "Lanekeeper agent CA",
                    "--valid-days",
                    "3650",
                ])),
            ),
            (
                "accepted_ca_init_options_in_another_order",
                text(&ca_init(&[
                    "--valid-days",
                    "1",
                    "--common-name",
                    "a",
                    "--kms-ref",
                    KEY_VERSION,
                ])),
            ),
            ("rejected_ca_init_without_options", text("ca-init")),
            (
                "rejected_ca_init_without_days",
                text(&ca_init(&[
                    "--kms-ref",
                    KEY_VERSION,
                    "--common-name",
                    "Lanekeeper agent CA",
                ])),
            ),
            (
                "rejected_ca_init_a_key_and_not_a_version",
                text(&ca_init(&[
                    "--kms-ref",
                    "projects/lk/locations/l/keyRings/r/cryptoKeys/k",
                    "--common-name",
                    "a",
                    "--valid-days",
                    "30",
                ])),
            ),
            (
                "rejected_ca_init_an_option_twice",
                text(&ca_init(&[
                    "--kms-ref",
                    KEY_VERSION,
                    "--kms-ref",
                    KEY_VERSION,
                    "--common-name",
                    "a",
                    "--valid-days",
                    "30",
                ])),
            ),
            (
                "rejected_ca_init_an_unknown_option",
                text(&ca_init(&[
                    "--kms-ref",
                    KEY_VERSION,
                    "--common-name",
                    "a",
                    "--valid-days",
                    "30",
                    "--force",
                ])),
            ),
            (
                "rejected_ca_init_days_out_of_range",
                text(&ca_init(&[
                    "--kms-ref",
                    KEY_VERSION,
                    "--common-name",
                    "a",
                    "--valid-days",
                    "3651",
                ])),
            ),
            (
                "rejected_ca_init_a_pasted_secret_as_the_name",
                text(&ca_init(&[
                    "--kms-ref",
                    KEY_VERSION,
                    "--common-name",
                    "hunter2:planted/secret",
                    "--valid-days",
                    "30",
                ])),
            ),
            (
                "rejected_ca_init_an_option_without_its_value",
                text("ca-init\0--valid-days"),
            ),
        ],
    );
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["hub.example".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let chain = cert.pem();
    let framing =
        |label: &str, body: &str| format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n");
    let with_key = |chain: &str, key: &str| [chain.as_bytes(), &[0], key.as_bytes()].concat();
    write_seeds(
        "hub_pem",
        &[
            ("rejected_empty", Vec::new()),
            ("rejected_not_pem", text("not pem at all\0not pem at all")),
            ("rejected_certificate_without_a_key", with_key(&chain, "")),
            (
                "rejected_key_with_a_garbage_body",
                with_key(&chain, &framing("PRIVATE KEY", "AAAAAAAAAAAA")),
            ),
            (
                "rejected_key_with_the_wrong_label",
                with_key(&chain, &framing("CERTIFICATE", "AAAA")),
            ),
            (
                "rejected_chain_with_a_garbage_certificate",
                with_key(&framing("CERTIFICATE", "AAAA"), &framing("PRIVATE KEY", "AAAA")),
            ),
            (
                "rejected_nine_certificates",
                with_key(&chain.repeat(9), &framing("PRIVATE KEY", "AAAA")),
            ),
            (
                "rejected_a_key_block_in_the_chain",
                with_key(&framing("PRIVATE KEY", "AAAA"), &framing("PRIVATE KEY", "AAAA")),
            ),
            (
                "rejected_bad_base64_in_the_chain",
                with_key(&framing("CERTIFICATE", "!!!not base64!!!"), ""),
            ),
            // A CA certificate secret holds one certificate and no key (T3): two, or one beside a private key, are refused.
            ("rejected_ca_two_certificates", with_key(&chain.repeat(2), "")),
            (
                "rejected_ca_certificate_beside_a_private_key",
                with_key(
                    &format!("{chain}{}", framing("PRIVATE KEY", "AAAAAAAAAAAA")),
                    &framing("PRIVATE KEY", "AAAAAAAAAAAA"),
                ),
            ),
        ],
    );
    println!("seeds rewritten");
}
