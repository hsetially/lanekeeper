# The agent's fuzz targets (S22)

The agent's own cargo-fuzz crate (decision A16). The root `fuzz/` crate belongs to prompt 01 and holds the domain's targets; this one is outside the root workspace too, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `agent_path` | one byte (the operation), then a path | on a root with planted symlinks (to `..`, to a file outside, to an absolute path, dangling, to a sibling directory), no operation changes anything outside the root, replaces a symlink, leaves a temporary file, or succeeds through a symlink |
| `agent_hub_message` | a protobuf `HubMessage` | decoding never panics; a configuration is clamped (scan 5 to 300 s, heartbeat 10 to 300 s, files at most 2 MiB) and its globs compile or fail cleanly; a file command gets exactly one answer of bounded size and touches nothing outside the root |
| `agent_cert_chain` | length-prefixed DER blobs | random bytes are never accepted as the agent's certificate (`ClientIdentity::verify`) |
| `agent_pem` | one byte, then PEM text | the private-key parser and the hub-CA parser never panic; an accepted key survives a PEM round trip; a generated key with damage applied is refused unless the damage left no trace |
| `agent_id_token` | text | a string accepted as a Google ID token is three non-empty base64url segments within the size limit |

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/agent/tests/corpus_replay.rs` include that file, so `cargo test -p agent` (and so `just verify`) replays every seed in `corpus/` with the same checks. `T9` adds `agent_spool_record`.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run agent_path --fuzz-dir crates/agent/fuzz \
    target/fuzz-corpus/agent_path crates/agent/fuzz/corpus/agent_path -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`. `just agent-fuzz` (added with `verify-02` in T7) runs every target and checks the lockfile with `cargo deny` and `cargo audit`.

The seeds hold no secret: the key seeds are damaged-key recipes applied to a key the target generates, and the certificates are public self-signed test certificates without their keys.
