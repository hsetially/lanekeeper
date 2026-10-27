# Blockers: prompt 18 (hub assembly)

Written in the FINAL step, 2026-10-27, by the implementer. The plan is frozen and approved; this file records what does not fit it. Nothing below has been worked around.

## B1. `just verify` is red: the S5 source scan of prompt 03b rejects the web listener's TLS key (plan G3)

**What fails.** `cargo test --workspace` (the third step of `just rust-verify`, so of `just verify`):

```
test ca_key_is_only_used_through_kms_signer_source_scan ... FAILED
thread 'ca_key_is_only_used_through_kms_signer_source_scan' panicked at crates/hub-platform/tests/ca_key_use.rs:192:5:
assertion `left == right` failed: a private key must never be built from bytes in the hub; the CA key lives in KMS
  left: [Finding { file: "web_tls.rs", line: 129, what: "`PrivateKeyDer::` in `decode`" }]
 right: []
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
error: test failed, to rerun pass `-p hub-platform --test ca_key_use`
```

It is the only failure of the workspace run (55 cargo invocations, 373 test binaries, 5,826 tests passed, 1 failed, 20 ignored as they were before). `just verify-18` does not run this test (its `hub-test` recipe runs `cargo test -p hub`, plus `hub-platform`'s `events_sse` and `xtask`'s `budgets_registry`), so the prompt's own gate is green and `just verify` is red.

**Cause.** `crates/hub-platform/tests/ca_key_use.rs` (prompt 03b, commit `badc418`) is the proof of S5 acceptance 3. It scans `crates/hub-platform/src` **and `crates/hub/src`** and fails on any constructor of a private key from bytes (`PrivateKeyDer::`, `KeyPair::from_pem`, `serialize_pem`, ...) outside an allowlist of exactly one function:

```rust
const ALLOWED: &[(&str, &str)] = &[("tls.rs", "leaf_certified_key")];   // a throwaway key generated in this process
```

Task T1 of this prompt (`656362b`) implements decision G3 of the approved plan: the web listener's certificate chain **and private key** come from two Secret Manager secrets, and `web_tls::decode` reads the key with `PrivateKeyDer::from_pem_slice` (`crates/hub/src/web_tls.rs:129`), wipes the PEM and the DER copy once rustls has its own, and reloads it on a timer. That is a private key built from stored bytes inside the hub, which the scan forbids. The scan was written for the CA key ("the CA key lives in KMS") and cannot tell the CA key from the listener's own TLS key. The plan's G3 row and its "Docs read" list name `tls.rs`, `session_auth.rs` and `ca/` of `hub-platform`, but not this test, and no task of T1 to T8 runs `-p hub-platform` tests other than `events_sse`, so it was not seen until the workspace run.

**What I did not do.** I did not change `crates/hub-platform/tests/ca_key_use.rs` (it is not in the approved `Extra-paths:`, and the guard would refuse it). I did not rename or move the call so that the scan's tokens no longer match (the scan is a security control; making the code invisible to it would defeat it and prove nothing). I did not weaken the scan.

**The exact change that would make it pass, if the human accepts G3 as built.** One line in `ALLOWED`, in the file above, and its doc comment (which says "the only places"):

```rust
const ALLOWED: &[(&str, &str)] = &[("tls.rs", "leaf_certified_key"), ("web_tls.rs", "decode")];
```

with the comment extended to say that `web_tls::decode` reads the web listener's own TLS key (never a CA key) from Secret Manager into memory, wipes the PEM and the DER copy at once, and that rustls keeps its own copy (the residual already recorded in `docs/threat-model.md`, "Hub assembly (18)", and in `web_tls::decode`'s doc comment). This needs `crates/hub-platform/tests/ca_key_use.rs` as an Extra-path of the plan, or a small follow-up PR on 03b's crate. A `docs/decisions.md` entry should say that S5's "no private key from bytes in the hub" means no CA key, and that the web listener's key is the one other key the hub holds (S6, S7).

**Alternatives for the human.**

- **A (recommended): allowlist `web_tls.rs::decode`** as above. Smallest change; keeps G3 as approved; the scan still fails on any other constructor, in any other function or file, and on any name that suggests a CA key.
- **B: change G3** to issue the web listener's certificate from the agent CA with `CertProfile::hub_server`, as `tls.rs::leaf_certified_key` does for the gRPC listeners (a key generated in this process, nothing stored). It removes the stored key, but the web certificate would then chain to the agent CA (browsers do not trust it, so the users' load balancer would have to accept it as a backend certificate, which plan G3 already says was not verified for Google's external HTTPS load balancer), and T1's `web_tls.rs`, its reload timer, its fuzz target `hub_pem` and its tests would change. That is a plan change, not a fix.
- **C: serve the web key through KMS** (a `rustls` `SigningKey` that calls `KmsSigner`). No key bytes in the hub at all, but it adds a KMS call to every TLS handshake and a new component; not in the plan.

**Recommendation.** A, with the decisions entry. Then add `cargo test -p hub-platform --test ca_key_use` to `hub-test` in the Justfile (this prompt owns that file), so that `just verify-18` shows this scan and the gate and `just verify` agree.

## Not blockers, but the human should know

- **The fuzz targets of this prompt had never run before this step.** The sandbox that wrote T1 to T8 had no nightly toolchain and no `cargo-fuzz`, so `hub-fuzz` and the `hub_*` invariants were only replayed on stable. Run for the first time (`nightly-2026-09-14`, `cargo-fuzz 0.13.2`, installed in this step), `hub_pem` crashed three times. All three are fixed in three `18/fix:` commits with a failing test or seed first; the fourth run of the unmodified recipe passed (all four targets, 30 s each, no crash). The findings: (1) `decode_ca_certificate` accepted a text with a second certificate whose `BEGIN` marker is glued to text before it on its line (the PEM grammar skips it), against its own "refuse and not skip" contract: product fix. (2) and (3) the harness check that "an error never repeats the secret's text" was unsound (a text that trims to nothing is contained in every message; a piece of the decoder's own fixed phrase is contained in it too): replaced by the sound check, that the error is one of the closed set of `PemError` values (the enum has no field), with a test that holds the list to the enum. (4) found by reading the invariants against the decoder after those: a CA certificate that ends before it begins was accepted although the harness asserts it never is: product fix, with a failing test first.
- **`cargo xtask bench-check` on the whole registry is red here only because of "registered but no result".** The ten budgets this prompt claims or must not regress (P2, P2.hub_head_moved_to_drift, P7.auth_composed, P7.auth_composed_cold, P11, P11.memory_mib, P12.audit_record, P12.hub_writes_queries, P12.steady_state_query_ms, P13) pass. 63 other budgets belong to other prompts' benchmarks, which were not run (the whole registry needs `just bench` on every crate; P10 needs the web build with Lighthouse), and two (P7.docs_search, P7.feature_status) have no benchmark yet (prompt 14).
- **The literal `just verify-18` cannot run in this sandbox:** one hub test binary is about 716 MB in the debug profile, 34 of them are 24 GB, and `cargo test -p hub --all-features` builds them all before it runs one. Each sub-recipe was run as written (`hub-tools-check`, `hub-lint`, `hub-fuzz`, `hub-bench`) and the three commands of `hub-test` were run, the first one target by target. See the evidence file.
