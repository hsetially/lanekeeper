//! T1 (S7, S10, S21): secrets stay out of `Debug`, errors and logs; the crate forbids unsafe code and never reads
//! Kubernetes Secrets.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::similar_names)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hub_identity::IdentityError;
use hub_identity::config::ClientAuth;
use hub_identity::oidc::login::{CallbackQuery, LoginSealer};
use serde_json::json;
use support::login_rig::{begin, query, rig, rig_with, sign_in_with};
use support::mock_oidc::{CLIENT_SECRET, TokenSpec, secret_auth};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn forbid_unsafe_present() {
    let lib = fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    let mut files = Vec::new();
    rust_sources(&crate_dir().join("src"), &mut files);
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        assert!(!text.contains("unsafe "), "{} uses unsafe", f.display());
        assert!(
            !text.contains("allow(unsafe_code)"),
            "{} allows unsafe",
            f.display()
        );
    }
}

/// Source files that may panic by design, and why. Nothing else under `src/` may.
/// - `testing.rs`: helpers behind the `route-table` feature that panic with every problem they find (its module comment
///   says so); the hub binary runs them in a test, never on a request.
/// - `http/tests.rs`: the body of `#[cfg(test)] mod tests;` in `http.rs`.
const PANIC_ALLOWED_FILES: [&str; 2] = ["testing.rs", "http/tests.rs"];

/// The production part of a source file: everything before the first `#[cfg(test)]` line (the test module closes each
/// file), without comment lines (doc comments may show `.unwrap()` in prose).
fn production_text(source: &str) -> String {
    source
        .lines()
        .take_while(|l| l.trim() != "#[cfg(test)]")
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every construct that rule 2 of `AGENTS.md` denies in non-test code, found by text. `unwrap_or*` and `expect_err` stay legal.
fn panic_sites(production: &str) -> Vec<&'static str> {
    const BANNED: [&str; 12] = [
        ".unwrap()",
        ".expect(",
        "panic!(",
        "todo!(",
        "unimplemented!(",
        "dbg!(",
        "allow(clippy::unwrap_used",
        "allow(clippy::expect_used",
        "allow(clippy::panic",
        "allow(clippy::todo",
        "allow(clippy::unimplemented",
        "allow(clippy::dbg_macro",
    ];
    BANNED
        .into_iter()
        .filter(|banned| production.contains(banned))
        .collect()
}

#[test]
fn no_unwrap_on_input_derived_data() {
    // S21 / AGENTS.md rule 2. The compile-time half is clippy: the crate takes its lint levels from the workspace, and the
    // workspace denies the panicking constructs. The proptests in `corpus_replay.rs` are the runtime half. This test pins
    // both of the settings the compile-time half depends on, and reads the production source for what slipped past them.
    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("[lints]\nworkspace = true"),
        "hub-identity must take its lints from the workspace"
    );
    let workspace = fs::read_to_string(crate_dir().join("../../Cargo.toml")).unwrap();
    for lint in [
        "unwrap_used",
        "expect_used",
        "panic",
        "todo",
        "unimplemented",
        "dbg_macro",
    ] {
        assert!(
            workspace.contains(&format!("{lint} = \"deny\"")),
            "the workspace must deny clippy::{lint}"
        );
    }
    let src = crate_dir().join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    let mut checked = 0;
    for f in files {
        let relative = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if PANIC_ALLOWED_FILES.contains(&relative.as_str()) {
            continue;
        }
        checked += 1;
        let found = panic_sites(&production_text(&fs::read_to_string(&f).unwrap()));
        assert!(
            found.is_empty(),
            "{relative} has panicking code outside its test module: {found:?}"
        );
    }
    assert!(
        checked > 30,
        "the scan must cover the crate, covered {checked} files"
    );
    // The two exceptions stay what they say: `testing.rs` is behind the feature, and `http/tests.rs` is a test module.
    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    assert!(lib.contains("#[cfg(feature = \"route-table\")]\npub mod testing;"));
    let http = fs::read_to_string(src.join("http.rs")).unwrap();
    assert!(http.contains("#[cfg(test)]\nmod tests;"));
}

#[test]
fn panic_scan_finds_planted_violations() {
    // The scan above is only worth having if it fires. Each plant is a production line a careless edit could add.
    for (plant, expected) in [
        ("fn f(x: Option<u8>) -> u8 { x.unwrap() }", ".unwrap()"),
        ("fn f(x: Option<u8>) -> u8 { x.expect(\"set\") }", ".expect("),
        ("fn f() { panic!(\"no\") }", "panic!("),
        ("fn f() { todo!() }", "todo!("),
        ("fn f() { dbg!(1); }", "dbg!("),
        (
            "#[allow(clippy::unwrap_used)]\nfn f() {}",
            "allow(clippy::unwrap_used",
        ),
    ] {
        assert!(
            panic_sites(&production_text(plant)).contains(&expected),
            "the scan missed {expected} in {plant}"
        );
    }
    // A violation inside the test module, in a comment, or spelled `unwrap_or` is not production code.
    let clean = "// x.unwrap() in prose\nfn f(x: Option<u8>) -> u8 { x.unwrap_or(0) }\n#[cfg(test)]\nmod tests { fn t() { Some(1).unwrap(); } }";
    assert!(panic_sites(&production_text(clean)).is_empty());
}

#[test]
fn no_kubernetes_secret_read_in_crate() {
    // The hub reads runtime secrets from Secret Manager through `SecretSource` (S8); the one file it reads from disk is
    // the projected service-account token. It has no Kubernetes client at all.
    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    for banned in ["kube", "k8s-openapi", "k8s_openapi"] {
        assert!(!manifest.contains(banned), "Cargo.toml mentions {banned}");
    }
    let mut files = Vec::new();
    rust_sources(&crate_dir().join("src"), &mut files);
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        for banned in ["kube::", "k8s_openapi", "/api/v1/namespaces", "api/v1/secrets"] {
            assert!(!text.contains(banned), "{} mentions {banned}", f.display());
        }
    }
}

#[tokio::test]
async fn secrets_are_redacted_in_debug_and_error_output() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    std::fs::write(&file, "assertion.secret.value").unwrap();
    let rig = rig_with(ClientAuth::Federated {
        token_file: file.to_string_lossy().into_owned(),
    })
    .await;
    let begun = begin(&rig, Some("/x")).await;
    let sealer = LoginSealer::from_key([5u8; 32]);
    let opened = sealer.open(&begun.cookie).unwrap();
    let verifier = opened.verifier.expose().clone();
    let sealed = sealer.seal(&opened).unwrap();
    let q = CallbackQuery::parse("code=CODE-VALUE-123&state=s").unwrap();
    let request = hub_identity::http::HttpsRequest::post_form(
        url::Url::parse("https://login.test/token").unwrap(),
        &[("client_secret", CLIENT_SECRET), ("code", "CODE-VALUE-123")],
    );
    let token = TokenSpec::valid("n").mint();
    let claims_text = {
        let keys = hub_identity::oidc::jwks::KeySet::default();
        format!(
            "{:?}",
            hub_identity::oidc::jwt::verify_with_keys(
                &token,
                &keys,
                &hub_identity::oidc::jwt::Profile {
                    issuer: "i".into(),
                    audiences: vec![],
                    tenant: None
                },
                0
            )
        )
    };
    let dumps = [
        format!("{rig_login:?}", rig_login = rig.login),
        format!("{opened:?}"),
        format!("{q:?}"),
        format!("{request:?}"),
        format!("{sealed:?}"),
        format!("{sealer:?}"),
        claims_text,
        format!("{:?}", secret_auth()),
    ];
    let secrets = [
        verifier.as_str(),
        "CODE-VALUE-123",
        CLIENT_SECRET,
        "assertion.secret.value",
        begun.cookie.as_str(),
        sealed.as_str(),
    ];
    for dump in &dumps {
        for secret in secrets {
            // `secret_auth()` names the secret in Secret Manager, which is configuration, not the value.
            assert!(!dump.contains(secret), "a secret leaked into: {dump}");
        }
    }
}

#[tokio::test]
async fn error_display_never_contains_input() {
    let marker = "ATTACKER-MARKER-9f3a";
    let rig = rig().await;
    // The marker rides in every field an attacker controls in a token: header and claims.
    let evil_header = |n: &str| {
        TokenSpec::valid(n)
            .header("kid", json!(marker))
            .header("alg", json!(marker))
    };
    let evil_kid = |n: &str| TokenSpec::valid(n).header("kid", json!(marker));
    let evil_iss = |n: &str| TokenSpec::valid(n).claim("iss", json!(marker));
    let evil_aud = |n: &str| TokenSpec::valid(n).claim("aud", json!(marker));
    let evil_tid = |n: &str| TokenSpec::valid(n).claim("tid", json!(marker));
    let evil_oid = |n: &str| TokenSpec::valid(n).claim("oid", json!(marker));
    let evil_nonce = |n: &str| TokenSpec::valid(n).claim("nonce", json!(format!("{marker}{n}")));
    let mut outcomes = Vec::new();
    outcomes.push(sign_in_with(&rig, evil_header).await);
    outcomes.push(sign_in_with(&rig, evil_kid).await);
    outcomes.push(sign_in_with(&rig, evil_iss).await);
    outcomes.push(sign_in_with(&rig, evil_aud).await);
    outcomes.push(sign_in_with(&rig, evil_tid).await);
    outcomes.push(sign_in_with(&rig, evil_oid).await);
    outcomes.push(sign_in_with(&rig, evil_nonce).await);
    // Query, cookie and state controlled by the caller.
    let begun = begin(&rig, None).await;
    rig.mock
        .set_id_token(TokenSpec::valid(begun.param("nonce")).mint());
    outcomes.push(
        rig.login
            .complete(&query(marker, marker), Some(&begun.cookie))
            .await,
    );
    outcomes.push(
        rig.login
            .complete(&query("c", begun.param("state")), Some(marker))
            .await,
    );
    outcomes.push(
        rig.login
            .complete(
                &query("c", begun.param("state")),
                Some(&URL_SAFE_NO_PAD.encode(marker)),
            )
            .await,
    );
    outcomes.push(
        match CallbackQuery::parse(&format!("code={marker}&code={marker}")) {
            Ok(q) => rig.login.complete(&q, Some(&begun.cookie)).await,
            Err(e) => Err(e),
        },
    );
    let errors: Vec<IdentityError> = outcomes.into_iter().filter_map(Result::err).collect();
    assert!(errors.len() >= 9, "most of these must fail, got {}", errors.len());
    for e in errors {
        let text = format!("{e} | {e:?}");
        assert!(!text.contains(marker), "input leaked into an error: {text}");
    }
}
