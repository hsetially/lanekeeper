# Fuzz targets (S22)

cargo-fuzz targets for the parsers at a trust boundary. Prompt 01 (T9) provides the skeleton and three targets; prompts 03b, 04, 14 and 17 add theirs.

| Target | Input | Invariants asserted |
|---|---|---|
| `nfs_path` | any UTF-8 string | an accepted `NfsPath` has no `..`, `.` or empty component, no NUL or control character, no backslash, no leading `/`, at most 1,024 bytes, and parses back to itself |
| `compare_ref` | any UTF-8 string | an accepted `CompareRef` displays as a string that parses back to the same value |
| `path_mapping_reverse` | first line an NFS path, further lines tenant ids | reverse mapping never panics; `forward(reverse(p)) == p` when it succeeds; a base repo path maps forward and back to itself |

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/domain/tests/corpus_replay.rs` include that file, so `cargo test -p domain` (and so `just verify`) replays every seed in `corpus/` with the same checks.

## Run

The fuzz crate is outside the root workspace (own `Cargo.lock`), so the main toolchain stays on stable 1.88. cargo-fuzz needs a nightly compiler and a C++ compiler. Versions are pinned in the `Justfile`.

```
rustup toolchain install nightly-2026-09-14 --profile minimal
cargo +nightly-2026-09-14 install --locked cargo-fuzz --version 0.13.2
just fuzz-smoke                 # 30 s per target; just fuzz_seconds=600 fuzz-smoke for longer
```

`just fuzz-smoke` fails, with the fix, when the toolchain or cargo-fuzz is missing; it never skips. It also checks that `fuzz/Cargo.lock` is complete (`--locked`) and passes `cargo deny` and `cargo audit`.

New inputs found during a run go to `target/fuzz-corpus/<target>/`, not into `corpus/`. Copy a useful input into `corpus/<target>/` by hand, with a descriptive name. A crash leaves its input in `fuzz/artifacts/<target>/`; add it to the seeds with a fix.

## Adding a target

1. Add the check to `invariants.rs` (or a new shared file), register it in `TARGETS`.
2. Add `fuzz_targets/<name>.rs` (copy an existing one), a `[[bin]]` entry in `Cargo.toml` and `corpus/<name>/` with at least `MIN_SEEDS` seeds.
3. Add the name to `fuzz_targets` in the `Justfile`.
4. `corpus_replay.rs` fails until all three agree.
