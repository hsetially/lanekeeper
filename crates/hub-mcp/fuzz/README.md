# The MCP crate's fuzz targets (S22)

The MCP crate's own cargo-fuzz crate, as the identity crate's is (decision A10 of plan 07). It is outside the root workspace, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `mcp_bearer_header` | bytes, split at newlines into up to four header values | parsing never panics and gives the same answer twice; no header is `Missing` and two or more are `Duplicate`; an accepted token is non-empty, at most 16 KiB, made of the `b64token` alphabet, and is the end of a `Bearer` value of exactly one header; a token over 16 KiB is never accepted; another scheme is never `Ok` |
| `mcp_pat_token` | text | parsing never panics; an accepted token is exactly 49 ASCII bytes with the `lkpat_` prefix, is the text it was parsed from, and is the one spelling of its 32 bytes; flipping one bit of the secret gives another valid token with another hash; the hash is stable and depends on the key; a key text that is accepted decodes to 32 to 1024 bytes, and a refused one gets one of three fixed sentences |

T1 added both. T2 adds `mcp_tool_args`, `mcp_cursor` and `mcp_http_body`, T3 `mcp_elicitation_result`, T6 `mcp_redactor`.

The targets do not depend on `hub-mcp`. They include the two source files they test, `src/auth/header.rs` and `src/auth/token.rs`, with `#[path]`, because a sanitizer build of the whole crate (rmcp, axum, sqlx and the hub platform) costs gigabytes for two small parsers. Both files are written to need nothing else of the crate, and `corpus_replay::the_two_files_the_fuzz_targets_include_need_nothing_of_the_crate` keeps them that way. Targets of T2 and later, which need more of the crate, will have to depend on it.

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs` include that file, so `cargo test -p hub-mcp` (and so `just verify-07`) replays every seed in `corpus/` with the same checks, and proptest feeds it random bytes.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run mcp_bearer_header --fuzz-dir crates/hub-mcp/fuzz \
    target/fuzz-corpus/mcp_bearer_header crates/hub-mcp/fuzz/corpus/mcp_bearer_header -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`. `just mcp-fuzz` runs every target and checks the lockfile with `cargo deny` and `cargo audit`.

The seeds hold no secret: the tokens are made-up constants that no server issued, and the key texts are bytes counted from zero.
