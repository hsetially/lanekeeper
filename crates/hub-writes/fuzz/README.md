# The write path's fuzz targets (S22)

The write path's own cargo-fuzz crate. The root `fuzz/` crate belongs to prompt 01 and holds the domain's targets; the agent, the identity crate and the hub platform each have theirs, as this crate does. It is outside the root workspace, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `writes_request_json` | bytes read as the JSON body of an edit, a revert, a restart, a draft, an approval and a rejection, and as the lines `path`, `expected`, `preview_id` of a delete and `status`, `swimlane`, `cursor`, `limit` of a list of proposals | reading never panics; an accepted request names a valid path and typed hashes, holds no more content than the input had, and survives a round trip through its canonical JSON (or lines) unchanged |
| `writes_upload_multipart` | a content type, a line end, and a multipart body, read as an upload form and as a preview form | reading never panics; an accepted request has a valid path, a file within the limit, an `Expected` that agrees with its fields, and survives a round trip through a canonical form unchanged |
| `writes_preview_id` | text read as a preview id presented with a fixed request at a fixed time | checking never panics; an id is accepted only if it is exactly the one the service issues for that request at the second it names, and that second is within the ten minutes before now |

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub-writes/tests/corpus_replay.rs` include that file, so `cargo test -p hub-writes` (and so `just verify`) replays every seed in `corpus/` with the same checks. Seed names say what they are for: `accepted_*` must still be accepted and `rejected_*` refused; a test fails if a seed stops meaning that.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run writes_request_json --fuzz-dir crates/hub-writes/fuzz \
    target/fuzz-corpus/writes_request_json crates/hub-writes/fuzz/corpus/writes_request_json -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`.

The lockfile is the repository's own, trimmed to what this crate builds, plus `libfuzzer-sys` 0.4.13 and `arbitrary` 1.4.2, the versions the other fuzz crates already pin (S19). `cargo deny --manifest-path crates/hub-writes/fuzz/Cargo.toml check` and `cargo audit --file crates/hub-writes/fuzz/Cargo.lock` check it.

## Seeds

`UPDATE_WRITES_SEEDS=1 cargo test -p hub-writes --test corpus_replay regenerate_the_seeds` rewrites them; without the variable the same test checks that the committed seeds are the ones the generator makes. They are deterministic and hold no secret. The preview-id seeds are tokens made with a fixed throwaway key at a fixed time.
