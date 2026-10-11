# The hub platform's fuzz targets (S22)

The hub platform's own cargo-fuzz crate (plan A17). The root `fuzz/` crate belongs to prompt 01 and holds the domain's targets, and `crates/agent/fuzz/` is the agent's; this one is outside the root workspace too, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `hub_agent_message` | a protobuf `AgentMessage` | decoding and validation never panic; an accepted message respects every size limit, re-encodes and converts back to the same value |
| `hub_join_csr` | a certificate signing request, DER | parsing never panics; an accepted request is at most 16 KiB, holds an uncompressed P-256 point and is signed by its own key (checked again by a second parser) |
| `hub_forward_request` | a protobuf `ForwardRequest` (`proto/replica.proto`), as a peer replica sends it | decoding and validation never panic; an accepted request names a valid swimlane id and a positive epoch, waits a bounded time, carries a command within every size limit, and re-encodes and validates to the same value |
| `hub_webhook_payload` | the JSON body of a GitHub `push` or `pull_request` delivery, and the inputs of the signature check | parsing never panics; an accepted push names a repository, a branch or tag and commit ids that are each well formed and bounded, and an all-zero `after` is a deletion; an accepted pull request has a positive number that fits the column; the signature check accepts a header only if it is exactly the HMAC-SHA256 of the body, checked against a second implementation of the HMAC |

| `hub_sentinel_message` | a protobuf `SentinelMessage`, as the sentinel on the NFS VM sends it | decoding and validation never panic; an accepted batch has at most as many records as the contract allows, each with a clean relative path, a known operation and bounded names; an accepted message re-encodes and converts back to the same value |

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub-platform/tests/corpus_replay.rs` include that file, so `cargo test -p hub-platform` (and so `just verify`) replays every seed in `corpus/` with the same checks. Seed names say what they are for: `accepted_*` must still be accepted, `rejected_*` refused, `skipped_*` skipped, `valid_*` and `bad_*` for requests; a test fails if a seed stops meaning that.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run hub_agent_message --fuzz-dir crates/hub-platform/fuzz \
    target/fuzz-corpus/hub_agent_message crates/hub-platform/fuzz/corpus/hub_agent_message -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`.

The lockfile is complete and uses only versions that the repository's other lockfiles already pin (S19); `cargo deny --manifest-path crates/hub-platform/fuzz/Cargo.toml check` and `cargo audit --file crates/hub-platform/fuzz/Cargo.lock` check it.

## Seeds

`UPDATE_HUB_SEEDS=1 cargo test -p hub-platform --test corpus_replay regenerate_the_seeds` rewrites them. The message, sentinel-message, forward-request and webhook-payload seeds are deterministic; the request seeds carry freshly made throwaway keys' public halves, so regenerating changes their bytes. The seeds hold no secret.
