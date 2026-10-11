//! S22: the fuzz seed corpora replay on stable, so a plain `cargo test` (and therefore `just verify`) exercises the same
//! invariants the fuzz targets assert, without a nightly toolchain or cargo-fuzz.
//!
//! The invariants live in `fuzz/invariants.rs`, which both this test and the fuzz targets include, so the two can never
//! drift apart. The fuzz crate is the identity crate's own (`crates/hub-identity/fuzz/`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../fuzz/invariants.rs"]
mod invariants;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

fn dir_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn replay(target: &str, check: invariants::Check) {
    let dir = fuzz_dir().join("corpus").join(target);
    let names = dir_names(&dir);
    assert!(
        names.len() >= invariants::MIN_SEEDS,
        "corpus for {target} has {} seeds, need at least {}",
        names.len(),
        invariants::MIN_SEEDS
    );
    for name in names {
        let bytes = fs::read(dir.join(&name)).unwrap();
        if let Err(why) = check(&bytes) {
            panic!("seed {target}/{name} violates an invariant: {why}");
        }
    }
}

#[test]
fn corpus_replay_identity_jwt() {
    replay("identity_jwt", invariants::identity_jwt);
}

#[test]
fn corpus_replay_identity_cookie() {
    replay("identity_cookie", invariants::identity_cookie);
}

#[test]
fn corpus_replay_identity_audit_event() {
    replay("identity_audit_event", invariants::identity_audit_event);
}

/// `identity_jwt` reaches the claim checks (`check_claims`). Random tokens fail the signature first, so the claim JSON and the time
/// logic are fed to `check_claims` directly: the input read as the claims, and the middle segment of the input read as a token.
/// These seeds are the ones that get there, one for each way through.
#[test]
fn identity_jwt_seeds_reach_the_claim_checks() {
    use hub_identity::TokenError;
    let outcomes = |name: &str| {
        let bytes = fs::read(fuzz_dir().join("corpus/identity_jwt").join(name)).unwrap();
        invariants::claim_outcomes(&bytes)
    };
    for name in [
        "claims_json_accepted",
        "claims_json_aud_list_float_exp",
        "claims_token_middle_segment",
    ] {
        assert!(
            outcomes(name).iter().any(Result::is_ok),
            "{name} no longer reaches an accepted claim set"
        );
    }
    for (name, expected) in [
        ("claims_json_expired", TokenError::Expired),
        ("claims_json_wrong_tenant", TokenError::WrongTenant),
        ("claims_json_future_nbf", TokenError::NotYetValid),
        ("claims_json_wrong_issuer", TokenError::WrongIssuer),
    ] {
        assert!(
            outcomes(name).iter().any(|r| r.as_ref().err() == Some(&expected)),
            "{name} no longer reaches {expected:?}"
        );
    }
}

/// The invariants on accepted claims are not vacuous: claims that break each one are reported.
#[test]
fn claim_invariants_report_what_they_are_meant_to() {
    use hub_identity::oidc::jwt::Claims;
    let good = Claims {
        issuer: "https://issuer.test".to_owned(),
        audience: vec!["aud".to_owned()],
        exp: 1_800_000_100,
        nbf: Some(1_799_999_000),
        nonce: None,
        tid: Some("TID".to_owned()),
        oid: None,
        subject: None,
        email: None,
        email_verified: None,
        preferred_username: None,
        name: None,
        scp: None,
    };
    assert_eq!(invariants::claim_invariants(&good), Ok(()));
    let mut c = good.clone();
    c.issuer = "https://elsewhere.test".to_owned();
    assert!(invariants::claim_invariants(&c).is_err(), "wrong issuer");
    let mut c = good.clone();
    c.audience = vec!["other".to_owned()];
    assert!(invariants::claim_invariants(&c).is_err(), "wrong audience");
    let mut c = good.clone();
    c.audience = Vec::new();
    assert!(invariants::claim_invariants(&c).is_err(), "no audience");
    let mut c = good.clone();
    c.tid = Some("other".to_owned());
    assert!(invariants::claim_invariants(&c).is_err(), "wrong tenant");
    let mut c = good.clone();
    c.tid = None;
    assert!(invariants::claim_invariants(&c).is_err(), "no tenant");
    let mut c = good.clone();
    c.exp = 1_700_000_000;
    assert!(invariants::claim_invariants(&c).is_err(), "expired");
    let mut c = good;
    c.nbf = Some(1_900_000_000);
    assert!(invariants::claim_invariants(&c).is_err(), "not yet valid");
}

/// Every target in `fuzz_targets/` has a corpus and a replay test, and the other way round.
#[test]
fn every_fuzz_target_has_a_corpus_and_a_replay() {
    let targets: BTreeSet<String> = dir_names(&fuzz_dir().join("fuzz_targets"))
        .into_iter()
        .map(|f| f.trim_end_matches(".rs").to_owned())
        .collect();
    let corpora = dir_names(&fuzz_dir().join("corpus"));
    assert_eq!(targets, corpora);
    let this =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus_replay.rs")).unwrap();
    for t in &targets {
        assert!(
            this.contains(&format!("fn corpus_replay_{t}")),
            "no replay test for {t}"
        );
    }
}

mod random_inputs {
    use proptest::prelude::*;

    use super::invariants;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn any_bytes_satisfy_the_jwt_invariants(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            prop_assert_eq!(invariants::identity_jwt(&data), Ok(()));
        }

        #[test]
        fn any_bytes_satisfy_the_cookie_invariants(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            prop_assert_eq!(invariants::identity_cookie(&data), Ok(()));
        }

        #[test]
        fn any_bytes_satisfy_the_audit_invariants(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            prop_assert_eq!(invariants::identity_audit_event(&data), Ok(()));
        }

        /// Text that is shaped like an audit event or a checkpoint object reaches the deeper branches of the parsers.
        #[test]
        fn json_shaped_text_satisfies_the_audit_invariants(
            seq in any::<i64>(),
            sig in "[A-Za-z0-9_-]{0,200}",
            head in "[0-9a-f]{60,66}",
            diff in proptest::option::of("[ -~]{0,64}"),
        ) {
            let object = format!(
                r#"{{"seq":{seq},"head":"{head}","signature":"{sig}","key_version":"v","created_at":1}}"#
            );
            prop_assert_eq!(invariants::identity_audit_event(object.as_bytes()), Ok(()));
            let diff = diff.map_or_else(|| "null".to_owned(), |d| format!("{d:?}"));
            let event = format!(
                r#"{{"at":{seq},"actor":{{"kind":"system","component":"c"}},"action":"file_edited","via":"ui","swimlane":null,"path":null,"hash_before":null,"hash_after":null,"diff":{diff},"github_login":null,"approval":null,"request_id":null,"attribution":null}}"#
            );
            prop_assert_eq!(invariants::identity_audit_event(event.as_bytes()), Ok(()));
        }

        #[test]
        fn path_like_text_is_judged_by_the_return_to_rule(s in "[/\\\\a-z?=&%.-]{0,40}") {
            prop_assert_eq!(invariants::identity_cookie(s.as_bytes()), Ok(()));
        }
    }
}
