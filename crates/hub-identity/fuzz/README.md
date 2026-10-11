# The identity crate's fuzz targets (S22)

The identity crate's own cargo-fuzz crate, as the agent's is (decision A16). It is outside the root workspace, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `identity_jwt` | text | parsing a token never panics and a token over 16 KiB is refused without being looked into; an accepted header is RS256 with a key id of 1 to 256 bytes; random input is never verified as a signed token; a key-set document that parses holds 1 to 32 keys |
| `identity_cookie` | text | `valid_return_to` accepts only a site-relative path; a login cookie that was not sealed with the key never opens; the callback query parser never panics |

| `identity_audit_event` | bytes | an audit event that parses is accepted or refused (`TooLarge`) by exactly the 64 KiB rule on its diff, and its canonical JSON parses back to an equal event and re-canonicalises to the same text; a stored row is judged by its hash alone, so the right hash passes and any single changed byte of the event, the predecessor or the hash fails; a checkpoint object read from the bucket survives a round trip and holds a signature of 1 to 128 bytes; a public key that is not a P-256 key is refused, and random bytes are never a valid checkpoint signature |

T3 added the `Cookie` header parser to `identity_cookie`; T5 added `identity_audit_event`.

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub-identity/tests/corpus_replay.rs` include that file, so `cargo test -p hub-identity` (and so `just verify`) replays every seed in `corpus/` with the same checks, and proptest feeds it random bytes.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run identity_jwt --fuzz-dir crates/hub-identity/fuzz \
    target/fuzz-corpus/identity_jwt crates/hub-identity/fuzz/corpus/identity_jwt -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`. `just identity-fuzz` runs every target and checks the lockfile with `cargo deny` and `cargo audit`.

The seeds hold no secret: the keys are random bytes that are not RSA keys, and no token in a seed was signed by anything.
