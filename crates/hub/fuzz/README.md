# The hub binary's fuzz targets (rule 13)

The hub binary's own cargo-fuzz crate (plan 18, T1). `crates/hub-platform/fuzz/` holds the platform's targets, `crates/hub-identity/fuzz/` the
identity module's, and so on; this one is outside the root workspace too, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `hub_settings` | lines of `NAME=value` (the `LK_*` environment) and one `FILE_CONTENT=...` line (what every `*_FILE` setting reads) | parsing never panics; an error is a variable's name and one of two fixed phrases (`is required`, `is not valid`) and never part of a value (S7, S10); an accepted value is inside its range, a web certificate reference is a valid secret name, a project id holds only what a resource name can, a Trusted Types name cannot inject into the CSP, and the database URL is not in the debug output of its settings. The same input is also read as the settings of `hub migrate` (T2), held to the same error rule, a time limit between 1 and 7200 seconds, and the migrator URL being the file's trimmed text and not in the debug output; the seeds named `*_migrator_*` exercise that side, and `corpus_replay.rs` pins the migrator's verdict for them |
| `hub_cli_args` | the arguments, separated by NUL bytes | parsing never panics; the parser agrees with a second statement of the grammar written from the usage text (`serve`, `migrate`, `help`); an error is one of two fixed phrases and never repeats an argument |
| `hub_pem` | the web certificate secret's text, a NUL byte, and the key secret's text | decoding never panics; an error is one of the fixed `PemError` words and never repeats the text; an accepted chain has one to eight certificates, its leaf is read again by a second parser and the expiry agrees, and the key was checked against the leaf |

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub/tests/corpus_replay.rs` include that file, so
`cargo test -p hub` (and so `just verify`) replays every seed in `corpus/` with the same checks. Seed names say what they are for:
`accepted_*` must still be accepted and `rejected_*` refused; a test fails if a seed stops meaning that.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
just hub-fuzz
cargo +nightly-2026-09-14 fuzz run hub_pem --fuzz-dir crates/hub/fuzz \
    target/fuzz-corpus/hub_pem crates/hub/fuzz/corpus/hub_pem -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash
leaves its input in `artifacts/<target>/`.

The lockfile is complete and uses only versions that the repository's root lockfile already pins (S19), except `libfuzzer-sys` and
`arbitrary`, which are the versions the other fuzz crates pin. `cargo deny --manifest-path crates/hub/fuzz/Cargo.toml check` and
`cargo audit --file crates/hub/fuzz/Cargo.lock` check it.

## Seeds

`UPDATE_HUB_SEEDS=1 cargo test -p hub --test corpus_replay regenerate_the_seeds` rewrites them. They hold no secret: the key bodies are garbage
between real PEM framing, and the one certificate is a public throwaway. There is no `accepted_*` seed for `hub_pem` because that would be
a private key in the repository; the accepted path is covered by a pair made in `corpus_replay.rs`, and the fuzzer finds it by mutation.
