# Review of prompt 05 (registry, indexes, incremental pipeline and read API)

Reviewer: independent `reviewer` agent, `prompts/REVIEW.md`, read-only. Reviewed HEAD `a3b29b4` against `origin/main` `777b590`.

## Round 1

VERDICT: CHANGES REQUESTED

The reviewer found no code defect. The two blocking items are the security docs and a gate it could not run.

### Blocking

1. `docs/threat-model.md:540`, `:541`, `:552`, `:554`, and the tests that pin them, `tests/threat_model.rs:88-91` and `:133-136`. The "(05)" section says the retention job and the `sync_windows` prune "are not built" and that "storage grows until the job exists".
   - Failing scenario: the T9 retention commits (`7ae27e5` to `dcd66a5`, D116-D119) came after the section's last commit (`7f824bd`, T10) and never touched it. The section is false about shipped code. It has no STRIDE row for the one component that deletes data irreversibly: blob GC by SQL under the app role's own DELETE grant.
   - Missing residuals, none of which is in the section: audit hashes whose blobs are gone (D116); GC deletes are unaudited (D118, A12); the plan statement reads all blobs on every run and is bounded only by `gc_scan_timeout` (`plan.rs:305`); the `sync_windows` prune versus late attribution: an observation reported more than 7 days after it was seen finds no window and stays out of band (D117); the missing-blob WARN that logs a hash (`read/content.rs:228`); the planner-statistics window after a bulk load (evidence open item 4); the blob endpoint under changed hub globs (non-blocking 6).
   - Also stale: D112 (`decisions.md:365`) says "the job is not built here".
   - Violates S24 and AGENTS rule 13.
   - Fix: rewrite `:540`, `:541` and `:552` to describe the job. Add STRIDE rows citing the real tests: `retention_races` (put between plan and delete, ingest not yet committed, referencing writer), `retention_columns`, `retention_security`, `retention_prune`. Add the residuals above. Update the pins in `tests/threat_model.rs`.
2. Process, not a code defect: `just verify-05` (registry-fuzz needs nightly and cargo-fuzz; registry-tools-check needs Docker), `just verify`, `cargo deny`, `cargo audit` (including the new `crates/hub-registry/fuzz/Cargo.lock`) and osv-scanner cannot run in this sandbox. The evidence file also leaves the verify-05 box unticked. The skill rule is that a gate the reviewer cannot run is blocking.
   - Fix: a green CI or human run of those, pasted into the PR. The fuzz targets were not run with libFuzzer; `corpus_replay` stands in.

### Non-blocking

1. `P7.settings_search` is honest under its statistic but its sample design is fragile (`benches/search_settings_all_swimlanes.rs:68`; header `:6-9`). 20 needles x 5 rounds makes 100 samples; the nearest-rank p95 is the 6th slowest. In the gate run the five slowest are one 2-character needle, "id": 195, 199, 206, 250 and 251 ms (it matches nothing, so it scans the whole index). In the reviewer's re-run the slowest five were again "id" (192 to 234 ms) and p95 was 63.4 ms (gate: 108.4 ms). p95 is therefore the slowest of the other 95 requests; the class that exceeds 200 ms is excluded by the 5% share; p99 and max are 250 ms. It fails only if at least 6 requests exceed 200 ms. It would breach if more than 5% of real queries were 1-2 character needles (type-ahead). Fix: add a minimum needle length of 3 at the edge (contract: openapi `minLength`) or register a separate budget for short needles, and say so in the PR. The 196.9 ms run is the pre-ANALYZE planner effect and does not repeat.
2. The gate's two bench fixes are legitimate. `6b5881c` binds the sixth parameter exactly as `read_drift` does (`SEVERITY_RANK_TOP`). `f5d5fc7` gives the bed a steady-state database, as autovacuum would; it does not hide a defect. The production window is real and not handled: after a bulk load and before autovacuum's first analyze, dense settings searches cost 100-160 ms a statement, and the app role cannot ANALYZE. It is in evidence open items 4-5 but in no doc: put it in the PR text and the threat model. Possible contract follow-up: per-table `autovacuum_analyze` settings in a migration. The bench was not re-run from T9 to the final step; that is how the 5-versus-6 parameter panic went unseen.
3. P12 evidence gap. `benches/steady_state_queries.rs:2` says "every query the read API makes", but `AUTO_NOTIFY_SQL`, `DRIFT_BY_PATH_SQL` and `DRIFT_PIVOT_SQL` are not timed. EXPLAIN snapshots exist for all three (`tests/snapshots/explain__read_*`), and the prompt's own Verify (EXPLAIN snapshots) is met. `DRIFT_BY_PATH_SQL` sorts the whole matching set on every page; it is the statement most likely to pass 50 ms on a swimlane with tens of thousands of drifted files, and it is unmeasured. Fix: add the three statements to the bench. Add a test that every `pub const *_SQL` of `read::` is in the bench or on an allow-list, and that runs the bench's statement list once at small scale. That would have caught the bind count.
4. Seven fixed-sleep "nothing happened" tests. Three are lock-wait checks: `tool_write_matching.rs:993`, `sentinel_sink.rs:683` and `attribution_job.rs:123`; each asserts `!run.is_finished()` after 400 ms and then asserts, after the lock is released, that the work completed, so the positive half carries the proof. A correct implementation can never fail any of the seven, so they cannot flake; a broken one fails them on an unloaded host and could pass only under heavy load. Worth tightening with a counter or `wait_until`: `pipeline_events.rs:316`, `read_corpora.rs:278` and `:298`, `severity.rs:542`.
5. D116(2) needs explicit human confirmation. The decisions stay inside owned paths and are not contract changes in disguise, apart from the two flagged below. D116(2): "referenced by an audit event" means a `blob_refs` row, not audit `hash_before`/`hash_after`. This narrows the prompt's T9 text, D78's first bullet and `domain-model.md:334-337`, none of which were amended. The reviewer judges it sound. The schema CHECK lists `audit_event` as a `blob_refs` kind and `hub-writes` already writes those rows (`crates/hub-writes/src/pipeline/execute.rs:252`), which makes the text "none does today" (`retention/plan.rs:23`) and "nothing adds one today" (D116) stale. D118: GC deletes by default; do not wire or enable the job before D116 and D118 are confirmed. Product behaviours to confirm: D98 (alerts only for nfs_ahead, conflict and untracked), D102 (4 h hold budget in 24 h), D103 (severity rules are raise-only, so an admin cannot lower or disable a default, although D76 says admin-configurable), D106 (hints and suffixes are evidence, not tenant members), D111 (A28 masking gap against S14b, which the threat model lists). D108 correctly flags the conflict between the port text (drift by path) and REST (by severity) as a contract question. D97: the human's approval for P7.text_search is recorded in D97 and `blockers.md:382-384`; only the description changed, from "across all swimlanes" to "steady state"; the cold case (p95 529 ms) is outside the budget.
6. The blob endpoint is not covered by the hub-glob defence in depth. `blob_policy.rs:30-34` tests only `file_state.denied`. It does not test the class or the hub's globs, while D104(3) and the threat-model row say the globs apply "wherever one file is decided". Scenario: the hub restarts with a new `extra_deny_globs` and the row has not yet been re-flagged by the agent. REST and the port withhold the file (`tests/denied_files.rs`, `a_hub_whose_own_globs_changed...`), but `GET /api/v1/blobs/{hash}` serves it to a Viewer who knows the hash. The window is seconds to minutes. Document it, or have `allowed()` also fetch the paths of current rows with that hash and apply the matcher. Also: if a denied file is deleted, its `file_state` row goes and the second EXISTS no longer refuses its old versions.
7. Acceptance item 3 holds with one pinned exception: `tests/http_contract.rs:701`. `SwimlaneSummary.tenants` has `minItems` 1, but an unadopted swimlane answers `[]`. The evidence lists it as open item 9. The human must choose: `minItems` 0 in the spec, or a placeholder tenant.
8. Commit order: T7 (attribution) landed after T8, T9 (first half) and part of T10. The plan order is T1 to T10 and AGENTS says "in the prompt's order". The blockers file explains the T1, T9 and T10 splits but not T7's late place. There is no behavioural effect.
9. Merge order and wiring. The branch needs the contract commits `352a25c`, `ede45af`, `b4c2351`, `6d9d512` and `933b668`. Those are not on main, so the PR cannot merge before the contract PR. Nothing is wired into `crates/hub`: there is no `Registry::build` or `registry_jobs()` as plan A8 said (D112). Eleven jobs plus the warm-up must be registered by hand, and a forgotten job fails silently. Security-relevant wiring: `RegistryBlobPolicy` must be installed on the blob endpoint, or denied hashes are served. The registry trusts the `User` it is handed, so the hub's `ActiveUser` guard must load the user per request. No adoption, mark-intentional or admin routes exist. Sentinel heartbeats do not reach the registry. All of this is written down in the evidence file.
10. Minor. `read/content.rs:228` now logs a WARN with a hash on every read of a collected version, which is an expected outcome after retention: use debug or a counter. Directory drift as the worst of its descendants (A25) is computed in SQL; `compare_tree`'s hash-map diff and the merge of shard results (`read/corpora.rs:57`) are compositions of engine output in the registry, covered by equivalence and property tests; no recomputation of drift, diff, effective config, grid or checks was found. The fuzz targets cover deltas, cursors, queries and sentinel paths; there is no target for cluster reports or sentinel batch validation, and the plan did not require one.

### Evidence re-run by the reviewer

- `cargo fmt --all -- --check`: exit 0.
- `cargo clippy -p hub-registry --all-targets --all-features -- -D warnings`: exit 0.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: exit 0.
- `cargo test -p hub-registry --lib --tests --no-fail-fast` with the sandbox Postgres, no `LK_REQUIRE_DOCKER`: exit 0, 77 binaries, 1159 passed, 0 failed, 3 ignored.
- `cargo test -p xtask`: exit 0, 162 passed, 0 failed, 1 ignored.
- `LK_REQUIRE_DOCKER=1 cargo test -p hub-registry --release --test adoption_rank_target --test corpora_equivalence -- --ignored --test-threads=1`: exit 0, 3 passed (the two binaries hold the three ignored tests; the other release binaries were not built, more than 80 minutes).
- Registry cut of `bench-check` (the 18 ids of the Justfile) on the existing gate outputs: exit 0, 18 pass, 0 fail, 0 unmet. Numbers equal the gate's: P1 244.2, P2 615.0, P5 27.4, P5.per_file 0.431, P6 11,697, adoption_rank 39.2, swimlane_list 1.8, file_tree 5.8, cached 1.6, cold 1.6, compare_api 28.1, grid 102.3, whole_swimlane_compare 24.9, settings_search 108.4, text_search 189.4, P12 15.0, P14 4,729, P15 min 1,547.
- Full `cargo xtask bench-check`: exit 0, 66 pass, 0 fail, 7 unmet of 73 (other prompts' budgets).
- Nearest-rank p95 recomputed from the raw samples of every hub-registry result file: all match the stored values.
- Re-ran only `search_settings_all_swimlanes`, once: exit 0, p95 63.4 ms; restored the gate's two result files, verified by diff.

### Could not run

`just verify-05` and `just verify` (no `just`); registry-fuzz, fuzz-smoke, nightly and cargo-fuzz; `cargo deny`, `cargo audit`, osv-scanner; Docker and testcontainers (the local Postgres stood in); web checks (`web/` is untouched); `cargo test --workspace` as one command (disk; the evidence file ran it crate by crate).

### Checked and found sound

- Scope: 05's own commits touch only `crates/hub-registry/**`, Cargo.toml and Cargo.lock, the Justfile, `plans/05*`, `docs/decisions.md`, `docs/threat-model.md`, `perf/budgets.toml` and `crates/xtask/tests/budgets_registry.rs`. Domain, proto, openapi, mcp-tools and design/handoff unchanged. Migrations only added (0007 and 0008, contract commits). No new third-party crate in the root lock.
- Plan chain: commit `57fac25` has the message "approved by a human"; the reviewer cannot verify the human. No later commit touches the plan file.
- Budgets: only registered flips, plus the one `P7.text_search` description edit. No threshold changed.
- Plan Verify tests: the 14 renamed tests exist. Insta snapshots are used only for EXPLAIN (107) and batch rows; the plan listed event and problem-body snapshots too, but they are covered by schema-walk tests.
- Renumbering: no stale D91, D92 or D109 citations in 05's code or docs.
- Denied files (D104): one function, `crate::denied::is_denied`, on tree, content (four sources), compare, effective, grid, served, the settings-search page, the text-search corpora, pickup and C10. A planted-word test with a control covers REST, the port, the blob endpoint, logs and metrics. The blob policy fails closed.
- Secrets: the settings index stores flagged values redacted, and `unscanned` redacts every value. The adoption scan stores file, line, length and rule only.
- SQL and paths: the only `format!`-built SQL takes the module's own time helpers, and a source scan enforces it. Paths go through typed parsers. Metric labels are closed enums. Logs carry ids and counts.
- Rules 1-5: no unwrap, expect, panic or todo in src, no `allow()` of them, `#![forbid(unsafe_code)]` present, every blob/Git/DB/agent call has a timeout, caches bounded, CPU work through `spawn_blocking` and rayon.
- Authorization: all 18 port methods and the 11 REST-only service methods reach `require_viewer` first. Routes are GET-only at Viewer through `GuardedRouter`. Adoption is Admin and marks are Operator, checked before any DB call.
- Retention: the deleter follows the 0008 contract (age tested in the same statement, `FOR UPDATE SKIP LOCKED`, delete as a second statement in the same transaction, the foreign key as a backstop, a catalogue test over every hash column). Dry run changes nothing, and the lease is checked before each write.
- T5 mapping: resolutions map to baselines consistently with the drift table.
- Tool-write registrations (D120): reachable only through the port, in the caller's transaction; matched by exact swimlane, path and hash; consumed once; expire at their TTL; cannot be dated into the future.
- Sentinel attribution (D123): hello needs a registered name and root; dedup by seq; paths map by component; attribution confined to the sentinel's own swimlane.
- Concurrency: the swimlane advisory lock is taken first and the audit chain last; pipeline commits are per swimlane; upserts write derived columns only; the incremental-equals-full proptest uses an independent model built on `engine::drift_state`.
- Benches measure the real paths through the router, with a real `PgBlobStore` and Postgres, at 40 swimlanes x 2,000 files. The P5 base bench asserts incremental equals full before timing. Units and statistics are right.
- Fuzz: all four targets exist and are listed in the Justfile; `corpus_replay` shares its invariants file with the targets; `tests/justfile_gate.rs` holds the lists together.
- Plan A8: the missing `Registry::build` is recorded in D112 and in the evidence file.

## Round 2

Reviewed HEAD `372cace` (five `05/fix:` commits and the evidence update after round 1).

VERDICT: APPROVE

No blocking finding other than the environmental one (round 1, item 2: the gates that cannot run in this sandbox). For everything an agent can check, the reviewer approves; the gates are for CI or a human.

### Blocking

None, apart from the environmental item.

### Non-blocking

1. `crates/hub-registry/src/blob_policy.rs:48-51`. The second `EXISTS` of `DENIED_HASH_SQL` has no LIMIT. For a popular hash with no denied file it probes every observation of that hash. Measured in a scratch Postgres: 100,000 ordinary observations of one hash, all with live `file_state` rows, cost about 20 to 37 ms per call, growing linearly; at design scale (at most 49 per hash) it is negligible. `tests/support/read_statements.rs:568-572` times `blob_policy_denied_hash` only with a hash nothing has had. The second `EXISTS` adds nothing: `HASH_PATHS_SQL` already reads `f.denied` through its join, and a hash with more than 1,000 observations is refused anyway. Fix: drop it, or add a worst-case entry to the P12 list.
2. `blob_policy.rs:226-229`. An undecided hash writes a WARN on every request, and any viewer who knows a popular hash can trigger it at request rate. Round 1's minor 10 treated the same kind of expected outcome as a counter plus a debug line; `blob_withheld` already counts it, so a debug line would do.
3. The P12 completeness claim is slightly too strong. The scan in `tests/read_statements.rs` sees only `pub const *_SQL` and `pub fn *_sql`. Four inline statements in `src/read` are invisible to it and not in the list: `search_settings.rs:179` and `search_text.rs:75` (`SELECT id FROM swimlanes ORDER BY id LIMIT 1000`), `swimlanes.rs:216` (existence of a swimlane) and `history.rs:118` (`read_is_version`, three `EXISTS`). All are key or prefix lookups. Fix: make them named constants, or have the scan flag inline `sqlx::query*("` literals.
4. Evidence consistency. The S17 row and non-blocking 6 say the blob policy takes "p95 0.3 to 1.8 ms"; the benchmark section says 2.6 ms typical, 12.0 ms for the most common content and 2.2 ms for the flag probe. Label or replace the first figure. The OpenAPI acceptance box is ticked although one deviation is pinned (`tenants` `minItems`); the PR text should say "with one pinned deviation".
5. Threat model. The "I / caller who knows a hash" row (`docs/threat-model.md:519`) cites `a_blob_the_store_does_not_have_gets_no_marker`, a settings-index test that says nothing about the endpoint. No residual says that an ordinary file's blob is also refused when a denied file has the same bytes (the any-denied rule is right; the over-refusal is a product-visible effect).
6. Still carried as PR-text follow-ups, none fixed, all disclosed: `P7.settings_search` is brittle (p95 108 against a budget of 200 ms; the sixth slowest of 100 samples is 196.9 in some runs); the seven fixed-sleep tests; `SwimlaneSummary.tenants` `minItems`; the T7 commit order; the contract PR that must merge first, and the wiring into `crates/hub`; human confirmation of D116 and D118 (retention deletes by default) before the job is wired.

### Round-1 findings and their disposition

| # | Disposition | Evidence |
|---|---|---|
| B1 threat model | Fixed (`26b4ec1`) | Section describes the shipped job; 36 STRIDE rows and 35 residuals; pins fail on a dropped row, lost proof, wrong letter or dropped word. |
| B2 gates | Environmental, not fixable here | See "Could not run". |
| NB1 settings_search needles | Left; documented follow-up (contract) | Residual in the threat model, evidence open item 5. |
| NB2 stats window | Documented, not built | Residual in the threat model, pinned. P12 re-run after the P12 commit. |
| NB3 P12 gap | Fixed (`794ed81`) | 45 unique statements shared by the bench and a test, with bind-count and run-once checks; binds spot-checked against the service. See non-blocking 3. |
| NB4 seven sleeps | Left; follow-up | |
| NB5 D116 and stale text | Stale text fixed (`9f4ec9a`) | Decisions stay with the human. |
| NB6 blob endpoint and hub globs | Fixed (`b547e1d`) | Mutation `f.path IS NULL` -> `false` fails two `blob_policy_globs` tests. |
| NB7 tenants `minItems` | Left; human choice, pinned by name | |
| NB8 T7 order | Left; no behavioural effect | |
| NB9 merge order and wiring | Left; documented | |
| NB10 WARN with a hash | Fixed (`a3e1e18`) | Unlabelled counter, debug line only, control in the test. |

### Re-ran

- `cargo fmt --all -- --check`: exit 0.
- `cargo clippy -p hub-registry --all-targets --all-features -- -D warnings`: exit 0.
- `LK_REQUIRE_DOCKER=1 cargo test -p hub-registry --lib --tests --no-fail-fast`: exit 0, 79 binaries, 1,178 passed, 0 failed, 3 ignored, 0 SKIPPED.
- `cargo test -p xtask`: exit 0, 162 passed, 1 ignored.
- `cargo check --workspace --all-targets --locked`: exit 0.
- Registry cut of `cargo xtask bench-check` on the existing results: exit 0, 18 pass, 0 fail, 0 unmet (P12 17.257 ms, P7.settings_search 108.4 ms, P7.grid 102.3 ms).
- P12 samples: recomputed p95 17.2565 ms over 900 samples (45 x 20); max 32.4 ms, so every statement is under 50 ms.
- Fixtures: the most common content has 49 files at target scale, as the code and evidence claim.
- Mutation: `f.path IS NULL` replaced by `false` in the deleted-file lookup of `HASH_PATHS_SQL`; two `blob_policy_globs` tests failed; restored, `git diff` empty.
- Scratch Postgres experiment for non-blocking 1 (one scratch database, dropped).

### Could not run

`just verify-05` and `just verify` (no `just`); `cargo deny`, `cargo audit` (including `crates/hub-registry/fuzz/Cargo.lock`) and `osv-scanner`; libFuzzer smoke (no nightly, no `cargo-fuzz`; `corpus_replay` stands in); Docker and testcontainers (the local Postgres stood in); web checks (`web/` is untouched); other crates' tests, the full `cargo test --workspace`, the release slow tests and all benchmarks (stored results were checked instead).

### Checked and found sound

- Blob policy fails closed on every path: probe error or timeout, paths-statement error, an unreadable row, a bad path or class, more than 1,000 sightings, and a closed pool all refuse. A hash shared by a denied and an ordinary path is refused; a deleted file's old versions are refused through its newest classified observation.
- No SQL from input; both statements bind only the hash and the limit. Logs carry fixed words only; no hash, path or content reaches a log or a metric.
- Indexes and plans: `file_observations_hash_idx`, `file_observations_history_idx` and `file_state_denied_hash_idx` exist in migrations 0001 and 0003; the EXPLAIN snapshot is a real `EXPLAIN` that also checks the generic plan.
- `denied.rs` changed in one doc line only.
- New tests have serving controls (globs; deleted file; file that stopped being denied; at-limit allowed and over-limit refused; failing lookup shown by revoking only the columns the paths statement reads).
- Scope: only `crates/hub-registry/**`, the two Extra-paths docs and the evidence file; no contract path changed; the plan file is untouched since `57fac25`.
- Evidence claims matched what was re-run: test counts, 108 snapshots, 19 new tests, P12 numbers and the unticked verify-05 box with its reason.
