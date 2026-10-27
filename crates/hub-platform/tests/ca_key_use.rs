//! Acceptance 3 (S5): the CA key is only ever used through `KmsSigner`, and a test fails if any code path loads a private CA
//! key.
//!
//! Three independent proofs, as the plan lists them:
//!
//! 1. a source scan of `crates/hub-platform/src` and `crates/hub/src` that fails on any constructor of a private key from
//!    bytes, outside a short allowlist of two functions in two files (neither holds a CA key), and on any way to name a CA
//!    private key;
//! 2. a type-level check that [`CaIssuer::new`] takes a signer, a key *name* and a certificate, and nothing that could be
//!    key bytes, and that the CA module has no `SecretSource`;
//! 3. a spy `KmsSigner`: every certificate signature the issuer made came from `sign_digest`, over the digest of the
//!    certificate's own to-be-signed bytes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use aws_lc_rs::digest::{SHA256, digest};
use domain::SwimlaneId;
use hub_platform::ca::{CaIssuer, CertProfile, parse_csr};
use ports::{KeyRef, KmsError, KmsSigner, Signature};
use support::ca::{AGENT_CA_KEY, TestCa, fresh_csr};
use support::clock::ManualClock;
use support::soft_kms::SoftKmsSigner;
use x509_parser::prelude::{FromDer, X509Certificate};

/// Calls that build a private key from bytes, or export one. Every one of them is a way to hold a key in the hub's memory.
const PRIVATE_KEY_TOKENS: &[&str] = &[
    "KeyPair::from_pem",
    "KeyPair::from_der",
    "KeyPair::from_pkcs8",
    "KeyPair::from_remote",
    "KeyPair::try_from",
    "from_pkcs8_der_and_sign_algo",
    "from_pem_and_sign_algo",
    "from_der_and_sign_algo",
    "EcdsaKeyPair::from_",
    "Ed25519KeyPair::from_",
    "RsaKeyPair::from_",
    "PrivatePkcs8KeyDer::",
    "PrivatePkcs1KeyDer::",
    "PrivateSec1KeyDer::",
    "PrivateKeyDer::",
    "serialize_pem",
    "serialize_der",
    "private_key_from",
];

/// Names a setting or a secret would have if the hub loaded a CA private key.
const CA_KEY_NAMES: &[&str] = &[
    "ca_private",
    "ca-private",
    "ca_key_pem",
    "ca-key-pem",
    "ca_key_file",
    "ca-key-file",
    "ca_key_der",
    "ca_signing_key_pem",
];

/// The only places a private key from bytes is allowed, each a key that is not a CA key. `(file, function)`:
///
/// - `tls.rs::leaf_certified_key`: the throwaway key generated in this process for the hub's own gRPC leaf certificate,
///   handed to rustls in the form it takes.
/// - `web_tls.rs::decode`: the web listener's own TLS key, which the hub reads from Secret Manager (prompt 18, plan G3). It
///   is read into memory, the PEM and the DER copy are wiped once rustls has its own, and the key is reloaded on a timer. It
///   is never a CA key: the two CA keys stay in KMS and are only ever used through `KmsSigner`, which the rest of this scan
///   and the two proofs below keep true. The residual (rustls keeps its own copy of this one key in memory) is recorded in
///   `docs/threat-model.md`, "Hub assembly (18)".
const ALLOWED: &[(&str, &str)] = &[("tls.rs", "leaf_certified_key"), ("web_tls.rs", "decode")];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    file: String,
    line: usize,
    what: String,
}

/// The text of `source` before its test module (by convention the last item of a file).
fn production_part(source: &str) -> &str {
    source.find("\n#[cfg(test)]").map_or(source, |at| &source[..at])
}

/// The name of the function that encloses `line` (1-based): the nearest `fn name` at or above it.
fn enclosing_function(lines: &[&str], line: usize) -> Option<String> {
    for text in lines[..line].iter().rev() {
        let trimmed = text.trim_start();
        let after = trimmed
            .strip_prefix("pub(crate) fn ")
            .or_else(|| trimmed.strip_prefix("pub fn "))
            .or_else(|| trimmed.strip_prefix("async fn "))
            .or_else(|| trimmed.strip_prefix("pub async fn "))
            .or_else(|| trimmed.strip_prefix("fn "));
        if let Some(rest) = after {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            return Some(name);
        }
    }
    None
}

/// Everything in `source` (the text of `file`, a path relative to a `src` directory) that this test forbids.
fn scan(file: &str, source: &str) -> Vec<Finding> {
    let lines: Vec<&str> = production_part(source).lines().collect();
    let mut findings = Vec::new();
    for (index, text) in lines.iter().enumerate() {
        let line = index + 1;
        let code = text.split("//").next().unwrap_or("");
        for token in PRIVATE_KEY_TOKENS {
            if code.contains(token) {
                let function = enclosing_function(&lines, line).unwrap_or_default();
                let allowed = ALLOWED
                    .iter()
                    .any(|(f, func)| file.ends_with(f) && function == *func);
                if !allowed {
                    findings.push(Finding {
                        file: file.to_owned(),
                        line,
                        what: format!("`{token}` in `{function}`"),
                    });
                }
            }
        }
        let lower = code.to_ascii_lowercase();
        for name in CA_KEY_NAMES {
            if lower.contains(name) {
                findings.push(Finding {
                    file: file.to_owned(),
                    line,
                    what: format!("`{name}` names a CA private key"),
                });
            }
        }
    }
    if file.starts_with("ca/") || file == "ca.rs" {
        for (index, text) in lines.iter().enumerate() {
            let code = text.split("//").next().unwrap_or("");
            if code.contains("SecretSource") || code.contains("SecretManagerSource") {
                findings.push(Finding {
                    file: file.to_owned(),
                    line: index + 1,
                    what: "the CA module must never read a secret".to_owned(),
                });
            }
        }
    }
    findings
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn scan_tree(src: &Path) -> Vec<Finding> {
    let mut files = Vec::new();
    rust_files(src, &mut files);
    assert!(!files.is_empty(), "{} has no sources", src.display());
    let mut findings = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        findings.extend(scan(&relative, &std::fs::read_to_string(&path).unwrap()));
    }
    findings
}

fn crate_src(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(name)
        .join("src")
}

#[test]
fn ca_key_is_only_used_through_kms_signer_source_scan() {
    let mut findings = scan_tree(&crate_src("hub-platform"));
    findings.extend(scan_tree(&crate_src("hub")));
    assert_eq!(
        findings,
        Vec::<Finding>::new(),
        "a private key must never be built from bytes in the hub; the CA key lives in KMS"
    );
}

#[test]
fn the_scan_fails_on_a_planted_private_key_constructor() {
    let planted = "pub fn load_ca(pem: &str) {\n    let key = rcgen::KeyPair::from_pem(pem);\n}\n";
    let findings = scan("ca/load.rs", planted);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].line, 2);
    assert!(findings[0].what.contains("KeyPair::from_pem"));
    // The same call in the allowlisted function of the allowlisted file is the one thing that passes ...
    let leaf = "fn leaf_certified_key(der: Vec<u8>) {\n    let k = PrivatePkcs8KeyDer::from(der);\n}\n";
    assert!(scan("tls.rs", leaf).is_empty());
    // ... and not in another file, or another function of that file.
    assert_eq!(scan("ca/mod.rs", leaf).len(), 1);
    let other = "fn something_else(der: Vec<u8>) {\n    let k = PrivatePkcs8KeyDer::from(der);\n}\n";
    assert_eq!(scan("tls.rs", other).len(), 1);
    // The web listener's own TLS key (never a CA key) is read from Secret Manager in `web_tls.rs`, function `decode`: the
    // second allowed place. It passes there ...
    let web = "fn decode(pem: &[u8]) {\n    let k = PrivateKeyDer::from_pem_slice(pem);\n}\n";
    assert!(scan("web_tls.rs", web).is_empty());
    // ... and not in another file, another function of that file, or the other allowed file ...
    assert_eq!(scan("ca/mod.rs", web).len(), 1);
    assert_eq!(scan("tls.rs", web).len(), 1);
    let web_other = "fn reload(pem: &[u8]) {\n    let k = PrivateKeyDer::from_pem_slice(pem);\n}\n";
    assert_eq!(scan("web_tls.rs", web_other).len(), 1);
    // ... and the allowance does not let a CA key be named there: the name check does not depend on the allowlist.
    let web_ca = "fn decode(pem: &[u8]) {\n    let ca_key_pem = PrivateKeyDer::from_pem_slice(pem);\n}\n";
    let findings = scan("web_tls.rs", web_ca);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].what.contains("names a CA private key"));
}

#[test]
fn the_scan_fails_on_a_planted_ca_key_name_and_on_a_secret_source_in_the_ca_module() {
    assert_eq!(scan("config.rs", "let a = settings.ca_private_key;\n").len(), 1);
    assert_eq!(
        scan("config.rs", "const NAME: &str = \"agent-ca-key-pem\";\n").len(),
        1
    );
    let planted = "use ports::SecretSource;\nfn key(s: &dyn SecretSource) {}\n";
    assert_eq!(scan("ca/mod.rs", planted).len(), 2);
    assert!(
        scan("gcp/secret_manager.rs", planted).is_empty(),
        "other modules may read secrets"
    );
}

#[test]
fn comments_and_test_modules_are_not_scanned() {
    let source = "fn a() {}\n// KeyPair::from_pem is how not to do it\n#[cfg(test)]\nmod tests {\n    fn t() { KeyPair::from_pem(\"\"); }\n}\n";
    assert!(scan("ca/mod.rs", source).is_empty());
}

/// `CaIssuer::new` takes a signer, a key name and the certificate. If a parameter that could carry key bytes were added,
/// this line would stop compiling.
#[test]
fn ca_issuer_new_takes_only_a_signer_a_key_name_and_a_certificate() {
    let constructor: fn(Arc<dyn KmsSigner>, KeyRef, Vec<u8>) -> _ = CaIssuer::new;
    let _ = constructor;
}

/// A signer that does what the real one does and writes down every call.
#[derive(Debug)]
struct SpyKms {
    inner: Arc<SoftKmsSigner>,
    calls: Mutex<Vec<([u8; 32], Vec<u8>)>>,
}

#[async_trait]
impl KmsSigner for SpyKms {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        let signature = self.inner.sign_digest(key, digest).await?;
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((digest, signature.as_bytes().to_vec()));
        Ok(signature)
    }
}

#[tokio::test]
async fn every_certificate_signature_came_from_sign_digest_over_its_own_tbs() {
    let key = KeyRef::parse(AGENT_CA_KEY).unwrap();
    let soft = Arc::new(SoftKmsSigner::new(&[&key]));
    let spy = Arc::new(SpyKms {
        inner: Arc::clone(&soft),
        calls: Mutex::new(Vec::new()),
    });
    let signer: Arc<dyn KmsSigner> = spy.clone();
    let ca = TestCa::over_signer(Arc::clone(&soft), signer, key, ManualClock::at_t0()).await;
    let before = spy.calls.lock().unwrap().len();
    assert_eq!(
        before, 2,
        "the ceremony signed the CA certificate and start-up probed once"
    );

    let mut leaves = Vec::new();
    for swimlane in ["sit1", "sit2", "sit3"] {
        let (_key, csr_der) = fresh_csr();
        let issued = ca
            .issuer
            .issue(
                &CertProfile::agent(&SwimlaneId::parse(swimlane).unwrap()),
                &parse_csr(&csr_der).unwrap(),
            )
            .await
            .unwrap();
        leaves.push(issued.chain_der[0].clone());
    }

    let calls = spy.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len() - before,
        3,
        "one KMS signature per certificate and no other use of the key"
    );
    for (leaf_der, (signed_digest, signature)) in leaves.iter().zip(&calls[before..]) {
        let (_, leaf) = X509Certificate::from_der(leaf_der).unwrap();
        let tbs_digest = digest(&SHA256, leaf.tbs_certificate.as_ref());
        assert_eq!(
            signed_digest.as_slice(),
            tbs_digest.as_ref(),
            "KMS was asked to sign the digest of the certificate's own to-be-signed bytes"
        );
        assert_eq!(
            leaf.signature_value.data.as_ref(),
            signature.as_slice(),
            "and the certificate carries exactly the signature KMS returned"
        );
    }
}
