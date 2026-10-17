# 20: Read-response masking (S14b)

| | |
|---|---|
| **Wave** | 2.5. Build when the human schedules it: before prompt 15, before prompt 08 T5 and before go-live. |
| **Depends on** | 03b, 04 and 05. Integrates with 06 (the marker refusal), 08 (T5) and 15 (the S14b check). |
| **You own** | `crates/hub-registry/src/mask/**`, `crates/hub-registry/src/http/dto.rs`, `crates/hub-registry/src/http/routes.rs`, `crates/hub-registry/src/blob_policy.rs`, `crates/hub-registry/src/read/api.rs`, `crates/hub-registry/src/read/cache.rs`, `crates/hub-registry/src/read/compare.rs`, `crates/hub-registry/src/read/content.rs`, `crates/hub-registry/src/read/etag.rs`, `crates/hub-registry/src/read/search_text.rs`, `crates/hub-registry/src/read/served.rs`, `crates/hub-registry/src/read/service.rs`, `crates/hub-registry/src/index/settings.rs`, `crates/hub-registry/src/adoption/mod.rs`, `crates/hub-registry/src/config.rs`, `crates/hub-registry/src/metrics.rs`, `crates/hub-registry/src/lib.rs`, `crates/hub-registry/Cargo.toml`, `crates/hub-registry/tests/**`, `crates/hub-registry/benches/**`, `crates/hub-registry/fuzz/**`, `docs/threat-model.md` |
| **Interfaces** | Changes the REST edge of the registry and `RegistryBlobPolicy`; no port changes. `RegistryRead` keeps returning raw text. Implements the `BlobPolicy` trait of `hub-platform` (unchanged). Consumes `engine::{secret_scan, ScanConfig, SecretMatch}`, `BlobStore` and `GitReader`. |
| **Security** | S10, S11, S13, S14b, S17, S21, S24 |
| **Performance** | P7 (`P7.file_content_cached`, `P7.file_content_cold`, `P7.compare_api`, `P7.text_search`, and the new scan rows), P11 |
| **Gate** | `just verify-20`. The Justfile holds an `exit 1` stub for it until T1 replaces it. |
| **Amended by** | D79, D95, D111 (its paragraph that says raw text is not masked), D128, and `contract/08-web` (CG7, option M1). These win over the task text. |
| **Blocking questions** | None blocks starting. Q20 (the scan thresholds are untuned; default: ship with the current thresholds, so files with a false positive are read-only in the web UI until Q20 is answered). |

## Objective

S14b says the web UI masks flagged values by default. `contract/08-web` fixed how (option M1: the registry masks at the REST edge, never in a port) and assigned the work to 05. Prompt 05 did not build it (D128), so today every signed-in Viewer receives flagged values in clear in file text, diffs, search lines, served text and blobs. The settings surfaces (effective config, grid, settings search) are already masked by the index (D95).

Build it in `crates/hub-registry`:

- every REST read response that carries file, diff, search or served text goes out with each flagged value replaced by `[REDACTED: N chars, rule]`, and with the `masks` list where the contract has one (`FileContent`, `ServedFile`);
- the ETag of a masked response carries the version of the scan configuration;
- `RegistryBlobPolicy` refuses the hash of a text file whose text the scan flags;
- a planted-key test proves it over every REST read operation.

The ports stay raw. hub-writes reads raw bytes through `RegistryRead::content` for edits, PRs, previews and reveal, and hub-mcp masks its own copy. Masking inside a port would change what they see, and `crates/ports/tests/web_contract_masking.rs` forbids it.

Prompt 05 is merged and owns this crate, so this prompt owns only the paths in the table above. The plan lists `docs/decisions.md`, `docs/interfaces.md` and `crates/xtask/tests/budgets_registry.rs` as Extra-paths, and requests the `perf/budgets.toml` rows as a contract change (the precedent of 05, 06 and 07: rows with `registered = false` first, flipped in the commit of each harness).

## Defaults the plan confirms

The investigation of 17 Oct 2026 left these open. The plan states an answer to each, and a human confirms them when approving it. Where the plan has no better answer, use the default.

1. **Where masking lives.** In the REST edge of `crates/hub-registry` (`FileContentDto`, `ComparisonDto`, the served and search edges, the blob policy). The ports return raw text, and a test pins it.
2. **Scan the whole blob first.** Scan the whole decoded text of a blob, then map the matches to lines. Never scan a cut search hit or a single hunk: a PEM body line is flagged only by a scan that saw the header, and a token that straddles the 512-byte cut of a hit can fall under the 32-character threshold.
3. **Scan version.** `ScanVersion` is a fingerprint of the `ScanConfig` thresholds, the names of the engine's rules and a constant in the registry, pinned by a golden test over the rule names so that a rule added in the engine fails the test and forces a bump. It lives in `crates/hub-registry`; no engine edit. The registry gets one source of `ScanConfig` for its own code (today `read/service.rs`, `index/settings.rs` and `adoption/mod.rs` each call `ScanConfig::default()`). hub-writes and hub-mcp keep theirs, and the divergence is recorded (Q20 will retune all of them). The settings index keeps the verdict of the day it was indexed (D95); a disagreement between an index marker and a file mask after a retune is a recorded residual, and re-indexing is out of scope because it needs a migration.
4. **Limits.** Decode and scan with the doubled input limit that hub-writes uses (`text_limits` in `crates/hub-writes/src/pipeline/validate.rs`), so that a 2 MiB Latin-1 or UTF-16 file whose decoded text exceeds `input_bytes` is not withheld on read while hub-writes scans it, and so that reveal accepts a setting exactly when a mask names it.
5. **Fail closed, per surface.** `getFileContent` and `getServedFile`: `content_withheld: true` and `text: null` (the contract). `compareFile`: a problem response, taken from the responses the operation documents; if none fits, stop (it is a contract change). Search: omit the hit and count it, and never fail the request, because an Editor can write a file that cannot be scanned (more than 100,000 matches) and one such file must not break every search. Blob policy: refuse. Cache the failure as well as the success.
6. **ETag.** The tag of every text or structured, non-withheld response carries the scan version and is computed before the blob is read, so a 304 is answered without a scan and a changed configuration is never answered from a stale 304 (the contract). Binary and withheld responses keep the hash. The compare and served tags follow the same rule.
7. **Lines and offsets.** `Mask.line` counts line feeds only (the contract), while the scan also ends a line at a lone CR. Convert each match to a byte offset in the whole text and derive the line from the number of `\n` before it, so that no contract change is needed. `start` and `end` are UTF-8 byte offsets into the masked text, on character boundaries. N in a marker is the flagged span's length in characters (the engine's `SecretMatch::marker`).
8. **`Mask.setting`.** Named only when exactly one setting's line range covers the line and the reveal operation would accept that setting (the rule of `view::marker` in `crates/hub-writes/src/proposals/view.rs`); otherwise absent, which means no reveal.
9. **Ranges.** `TextHit.ranges` and the `InlineRange` offsets index the masked text. Remap them through the replacement table; a range that overlaps a marker clamps to the marker.
10. **Oracles.** Search and compare still tell a reader whether a hidden value matches a pattern or equals another (open items 1 and 3 of `docs/interfaces.md`; the contract does not claim to close them). Default: build as the contract says (mask the hits; compare equality stays) and record both as residual risks with a decision. If the plan instead masks the search corpus before matching, which closes the first, the port returns masked lines and the contract wording changes: stop and escalate for a human answer.
11. **Blob policy.** `RegistryBlobPolicy::new` takes a `BlobStore`. On a miss it reads the blob, scans it and caches the verdict by (hash, scan version) in the same cache. It refuses any text or structured hash whose scan finds a match or cannot finish, and skips binary (the file class is already in the sightings). The existing rule stays: a hash is refused if any path that has had it is denied. The `BlobPolicy` trait in `hub-platform` is unchanged. Residual: bytes served before this ships stay in browser caches for up to a year (`immutable`).
12. **Budgets.** No row measures the scan today. The plan proposes thresholds from a measurement at target scale (40 swimlanes, about 80,000 places, about 23,000 text blobs, files up to 2 MiB). The benchmarks live in `crates/hub-registry/benches` and plant secrets in-process, because the fixtures plant none. "Measured with masking on" means `P7.file_content_cached`, `P7.file_content_cold`, `P7.compare_api` and `P7.text_search`, plus new rows for the scan, a planted-secret compare and the blob policy. (`docs/interfaces.md` says `P7.compare`; that row is the engine's benchmark, and the registry's is `P7.compare_api`.)
13. **Order with 06.** The refusal of a write that holds a marker (`redaction_marker`, prompt 06) is not built. This prompt does not need it to start, but go-live needs both: until it lands, a client that posts the masked text of `getFileContent` to `editFile` overwrites the secret with its marker. The threat model says so.

## Tasks

### T1: Scan source, scan version and limits (S13, D95)

- One scan configuration and one limits rule for the registry's own code, built once in `RegistryConfig` and handed to `read/service.rs`, `index/settings.rs` and `adoption/mod.rs` (default 3 and 4).
- `ScanVersion`, with the golden test over the engine's rule names.
- `verify-20` replaces its stub. It lists the registry's existing sub-recipes (`registry-tools-check`, `registry-lint`, `registry-test`, `registry-slow-test`, `registry-fuzz`, `registry-bench`), and `crates/hub-registry/tests/justfile_gate.rs` is extended so that the new fuzz target and the new budget ids are in `registry_fuzz_targets` and `registry_budgets`.

**Verify:** `scan_version_changes_when_a_threshold_or_a_rule_changes`, `the_registry_has_one_scan_config` (a source scan: no `ScanConfig::default()` in `crates/hub-registry/src` outside the one constructor) and `a_latin1_file_above_the_input_limit_is_scanned_as_hub_writes_scans_it`.

### T2: The masker (S14b, S11)

- A pure module in `crates/hub-registry/src/mask/`, with no I/O, and every output bounded. `apply(text, matches)` returns the masked text, the `masks` (at most 1,000, in start order, while the text is masked in full) and the replacement table. `mask_lines` does the same for the lines of one side of a diff. `remap_ranges` moves ranges through the table.
- Offsets are UTF-8 bytes of the masked text. Lines end at a line feed. A private key block gives one marker and one mask per line.
- Fuzz target `registry_masker` with these invariants: `text[start..end]` is a marker, boundaries fall on characters, offsets increase, no matched span survives in the masked text, and the output length is bounded.

**Verify:** property tests `masks_name_exactly_their_markers` and `offsets_survive_non_ascii_text`; golden tests for CRLF, a lone CR, text decoded from UTF-16, a 27-line private key and 1,001 matches; `just registry-fuzz` runs the target.

### T3: Scan cache and its cost (P7, P11; rule 5)

- A `moka` cache keyed by (content hash, scan version), holding the matches or `Unscanned`, weighted in bytes, with `scan_cache_bytes` in `RegistryConfig` (range-validated like the other cache sizes), a `CacheName` variant, and counters labelled by cache name and outcome only (keys name files, S10).
- The scan runs on rayon inside `spawn_blocking`, behind a bounded semaphore, and concurrent requests for one hash share one scan.
- Benchmark `crates/hub-registry/benches/mask_scan.rs` at target scale with planted secrets, including a private key block and a file with many matches. Its rows are requested as a contract change with `registered = false`, and flipped here.

**Verify:** `one_scan_per_hash_and_version_under_concurrency`, `a_failed_scan_is_cached_too`, `the_scan_cache_is_bounded_by_bytes`; `cargo xtask bench-check` for the new rows.

### T4: File content and served text (S14b, D79)

- `FileContentDto` gets `masks` and serves the masked text; the content ETag carries the scan version, and the 304 is answered before the blob is read (`read/api.rs`). `ServedFileDto` wraps `domain::ServedFile` and adds `masks` (without `setting`). Fail closed is the existing withheld shape. `Mask.setting` follows default 8.
- `RegistryRead::content` stays raw.

**Verify:** planted-key tests for `getFileContent` from each source (nfs, baseline, blob, git) and for `getServedFile`; `a_changed_scan_configuration_never_answers_a_stale_304`; `the_port_returns_raw_bytes_for_hub_writes`; `P7.file_content_cached` and `P7.file_content_cold` with masking on.

### T5: Compare (S14b; P7.compare_api)

- Mask at the REST edge in `ComparisonDto::from`, using the scan results of both sides next to the hunks: the hunk lines of both sides and the context lines are masked, and `inline_changes` is remapped. `setting_changes` is untouched (already markers; a changed secret must still not look unchanged, A28). The shared diff cache stays raw, so the port and hub-mcp are unaffected. The compare tag carries the scan version. An unscannable side follows default 5.

**Verify:** a planted-key compare test across same and different content; `inline_changes_index_the_masked_text`; `P7.compare_api` with masking on, within 100 ms at target scale.

### T6: Text search (S14b; P7.text_search)

- An internal registry variant of the search returns each hit with its blob hash. The REST edge scans the whole blob (cached), maps the matches to the hit's line, masks the cut text and remaps `ranges`. An unscannable file is omitted and counted. The port stays raw. The oracle is recorded (default 10).

**Verify:** `a_token_straddling_the_512_byte_cut_is_masked`, `a_private_key_body_line_hit_is_masked`, `ranges_index_the_masked_text`, `an_unscannable_file_does_not_fail_the_search`; `P7.text_search` in steady state with masking on, within 400 ms.

### T7: Blob policy (S14b, S17)

- As default 11. The counter `registry_blob_withheld_total` keeps counting, without hash or path.

**Verify:** `the_blob_policy_refuses_a_flagged_text_blob`, `a_binary_blob_is_not_scanned`, `an_unscannable_text_blob_is_refused`; the existing tests `blob_policy.rs`, `blob_policy_globs.rs`, `denied_files.rs` and `blob_double.rs` updated for the new constructor and still green.

### T8: Planted-key matrix (S14b, S10)

- A test over every GET operation of the registry's REST API, complete by construction: it walks `api/openapi.yaml` (with `tests/support/openapi.rs`, as `route_table.rs` does), and each operation is in the matrix or says why it carries no text (`compareTree`, `listFindings`, and so on). The key is built at run time from parts, as `crates/hub-mcp/tests/mcp_auth/planted_key.rs` does, and the test checks for the key, for its body without the prefix, and for the marker. It also searches captured log lines and the metrics exposition.

**Verify:** `cargo test -p hub-registry --test planted_key_rest`.

### T9: Threat model, decisions and interfaces (S24)

- Fill in the registry section of `docs/threat-model.md`: the A28 residual and gap 1 are replaced by the masking rows, with the oracles, the index-versus-file disagreement, the year-long browser cache and the write round trip (default 13) as residual risks. Update the needles that `crates/hub-registry/tests/threat_model.rs` pins for the old wording in the same commit.
- A new decision supersedes the paragraph of D111 that says raw text is not masked, and D128. `docs/interfaces.md`: the status of 05's row and of open items 1 to 3. `crates/xtask/tests/budgets_registry.rs`: the registered set.

**Verify:** `cargo test -p hub-registry --test threat_model`, `just verify-20`, `just verify`, `cargo xtask bench-check`.

## Acceptance (all required)

- [ ] `just verify-20` passes, and every P# item owned here is within budget.
- [ ] No value that the scan flags appears in any REST read response of the registry, from any source and on any surface, except in `POST /files/reveal`: the planted-key matrix is complete by construction over every GET operation.
- [ ] Every mask meets the contract: `text[start..end]` is exactly a marker; offsets are UTF-8 bytes of the masked text on character boundaries, in start order; `line` counts line feeds; at most 1,000 masks while the text is masked in full.
- [ ] `TextHit.ranges` and the `InlineRange` offsets index the masked text.
- [ ] The ports return raw text, and a test proves it for the calls that hub-writes makes.
- [ ] The ETag of a masked response carries the scan version, a 304 is answered before the blob is read, and a changed scan configuration is never answered from a stale 304.
- [ ] Failure is closed on every surface: withheld for content and served, omitted and counted for search, refused for the blob; one unscannable file cannot break a search.
- [ ] `getBlob` never serves a text file whose text the scan flags; the binary and denied rules are unchanged.
- [ ] `P7.file_content_cached`, `P7.file_content_cold`, `P7.compare_api` and `P7.text_search` are within budget with masking on, at target scale with planted secrets; the new rows are registered; `cargo xtask bench-check --strict` passes; the scan cache and the work in flight are bounded (P11).
- [ ] No path, hash or content reaches a log line, an error or a metric label of the new code (S10).
- [ ] The fuzz target `registry_masker` runs in `just registry-fuzz`.
- [ ] The two oracles and the disagreement between index markers and file masks after a retune are recorded in the threat model with a decision, not left silent.
- [ ] D111's paragraph and D128 are superseded by a new decision, and the threat-model needles are updated with the text.

## Stop and escalate if

- the scan cannot meet a P7 budget at target scale with this approach: include the profiles and the cold-scan numbers; do not drop masking, and do not make a cache unbounded, to fit;
- a mask cannot be produced as the contract describes it (`Mask.line` against a lone CR that the default 7 does not cover, a problem code for compare that the operation does not document, a `Mask.setting` that reveal would refuse): the fix is a contract change, so write it into the plan;
- persisting a verdict or a scan version needs a migration, or sharing the scan source needs an engine edit: both are decisions for a human;
- hub-writes' or hub-mcp's scan configuration has to change for reveal to agree with the masks: record it and do not edit their crates;
- a marker form of hub-writes that does not match the contract's pattern (`[REDACTED: the file could not be scanned]` in `proposals/view.rs`) reaches a REST read: report it to the owner of 06;
- the plan's choice for the oracles is to mask the search corpus, which changes what a port returns;
- a threshold cannot be tuned without a real tenant file: use the synthetic generator, and never commit real config (rule 12).

## Out of scope

The refusal of a write that holds a marker (06). The web's masking test and the switch-off of edit mode (08, T5). hub-mcp's own redactor (07). Masking of docs (14). Mounting the registry in `crates/hub` (18). Re-indexing the settings index (needs a migration). Retuning the thresholds (Q20). Engine changes. Closing the two oracles, unless the plan chooses to.
