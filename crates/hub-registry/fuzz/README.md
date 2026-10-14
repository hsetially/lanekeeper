# The registry's fuzz targets (S22)

The registry's own cargo-fuzz crate, as the agent's, the identity crate's, the platform's and the engine's are. It is outside the root workspace, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `registry_delta` | bytes read as a program: one to four messages of a scan delta, each with files, removals and skips | the checks a scan delta passes before the hub stores anything never panic and agree with an oracle written from the rules (the first bad entry decides the error); an accepted message equals the entries given; content is kept only for an entry that is not denied, and hashes to its entry; a denied entry keeps no content; no time is out of range; the totals of a logical delta never pass a bound; the plan for the versions of a path chains and never moves the state back |

T6 adds `registry_cursor` and `registry_query`, and T7 adds `registry_sentinel_path`.

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub-registry/tests/corpus_replay.rs` include that file, so `cargo test -p hub-registry` (and so `just verify`) replays every seed in `corpus/` with the same checks, and proptest feeds it random bytes.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run registry_delta --fuzz-dir crates/hub-registry/fuzz \
    target/fuzz-corpus/registry_delta crates/hub-registry/fuzz/corpus/registry_delta -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`. `just registry-fuzz` runs every target and checks the lockfile with `cargo deny` and `cargo audit`.

## The input of `registry_delta`

A byte stream, zero after its end. The first byte is the number of messages minus one (modulo 4). Each message: the counts of files (modulo 7), removals and skips (modulo 3), then the files, then the removed paths and the skipped paths. A path is a length byte (modulo 24, plus one) and that many bytes, each an index into the alphabet `ab/./-_kepmrivt01` and NUL, so that traversal, empty components and the deny globs (`*.pem`, `*private*`) all occur. A file is its path, a flags byte (denied, has bytes, hash true, size true, and two bits for the time: sane, negative, beyond year 9999, a year ahead), a content length (modulo 40) and the content, and a jitter byte. After the messages come the bytes that describe the stored state of the path the plan is run against.

The seeds hold no secret: the "keys" are the words `key` and `-----BEGIN-----`.
