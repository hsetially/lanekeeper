# The MCP crate's fuzz targets (S22)

The MCP crate's own cargo-fuzz crate, as the identity crate's is (decision A10 of plan 07). It is outside the root workspace, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `mcp_bearer_header` | bytes, split at newlines into up to four header values | parsing never panics and gives the same answer twice; no header is `Missing` and two or more are `Duplicate`; an accepted token is non-empty, at most 16 KiB, made of the `b64token` alphabet, and is the end of a `Bearer` value of exactly one header; a token over 16 KiB is never accepted; another scheme is never `Ok` |
| `mcp_pat_token` | text | parsing never panics; an accepted token is exactly 49 ASCII bytes with the `lkpat_` prefix, is the text it was parsed from, and is the one spelling of its 32 bytes; flipping one bit of the secret gives another valid token with another hash; the hash is stable and depends on the key; a key text that is accepted decodes to 32 to 1024 bytes, and a refused one gets one of three fixed sentences |
| `mcp_tool_args` | one byte (the tool, modulo 27) and JSON | parsing the arguments of any of the 27 tools never panics and gives the same answer twice; an unknown argument is `Invalid` and nothing is accepted with one; an accepted call has every required argument and its tool is the one asked for; `Missing` names a required argument that is absent and `TooLarge` follows only `content` over 2 MiB; an error is one of three fixed sentences; a present `null` is refused (except the PR `body`); the `Debug` of accepted arguments shows no `content` or `body`; every accepted path is relative and plain (docs paths absolute and plain) |
| `mcp_cursor` | a tool name, a newline, and a cursor | decoding never panics and gives the same answer twice; a cursor that decodes is exactly what `encode` makes for its parts, was made for the tool it was read for, is at most 512 characters and skips at most 200; `decode(encode(x)) == x` and an encoded cursor uses the base64url alphabet only; a cursor made for one tool is refused for another |
| `mcp_http_body` | bytes | as the body of a `POST /mcp` with valid credentials and a live session, through the whole service on fakes: an answer within 5 seconds with a status in 200, 202, 400, 404, 406, 413, 415, 422, 429, 503 (never 401, 403 or another 5xx); a 200 is an event stream or JSON; at most 512 KiB come back and the first message within 2 seconds; afterwards a `ping` on the same session is still answered |

T1 added the first two, T2 the next three. T3 adds `mcp_elicitation_result`, T6 `mcp_redactor`.

The targets of T1, `mcp_tool_args` and `mcp_cursor` do not depend on `hub-mcp`. They include the one source file they test with `#[path]` (`src/auth/header.rs`, `src/auth/token.rs`, `src/tools/args.rs`, `src/output/cursor.rs`), so each is built from that file and `domain`, `serde`, `serde_json`, `base64` and `thiserror` only. `corpus_replay` keeps these files from reaching for the rest of the crate. `mcp_http_body` is the exception: it sends bytes through the whole service, so it depends on `hub-mcp` (with its `fakes` feature) and on what it pulls in (rmcp, axum, sqlx, the hub platform). A sanitizer build of that costs gigabytes, and it is shared by all targets of this crate, because cargo builds the dependencies of a package once for all its binaries.

The invariants are in `invariants.rs` (T1's two targets), `invariants_tool_args.rs`, `invariants_cursor.rs` and `invariants_http.rs`. Both the fuzz targets and the stable test `crates/hub-mcp/tests/mcp_auth/corpus_replay.rs` include them, so `cargo test -p hub-mcp` (and so `just verify-07`) replays every seed in `corpus/` with the same checks, and proptest feeds them random input. The service of `mcp_http_body` is rebuilt every 200 inputs, so that sessions opened by bodies that happen to be valid `initialize` requests do not pile up toward the caps.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run mcp_bearer_header --fuzz-dir crates/hub-mcp/fuzz \
    target/fuzz-corpus/mcp_bearer_header crates/hub-mcp/fuzz/corpus/mcp_bearer_header -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`. `just mcp-fuzz` runs every target and checks the lockfile with `cargo deny` and `cargo audit`.

The seeds hold no secret: the tokens are made-up constants that no server issued, and the key texts are bytes counted from zero.
