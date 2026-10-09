# 04: Fact engine

| | |
|---|---|
| **Wave** | 1 |
| **Depends on** | 01 |
| **You own** | `crates/engine/**`, the engine fuzz targets in `fuzz/` |
| **Interfaces** | The engine API in `docs/interfaces.md`. Pure functions: no I/O, no tokio, no database. |
| **Security** | S11, S13, S21, S22 |
| **Performance** | P5, P6, P7 (compare, grid, settings, text search) |
| **Gate** | `just verify-04` |

## Objective

The engine is the single source of facts. It must be:

- **deterministic:** byte-identical output for identical input;
- **fast:** within the budgets above;
- **robust against hostile input:** every parser is fuzzed.

Every view in the UI and every MCP answer is built from these functions.

## Tasks

### T1: Bytes and classes

- `classify`: `structured`, `text` or `binary`.
- Detect encoding, BOM and line-ending style.
- `normalise_eol` and `apply_eol`.

**Verify:** property test that `apply_eol(normalise_eol(x), style(x)) == x` for CRLF and LF files with or without a BOM.

### T2: Parsing, validation and flattening

**Parsers.** A maintained YAML crate (not serde_yaml), chosen by benchmarking two candidates on the 6,000-line fixture. Record the choice and the numbers in `docs/decisions.md`. The parser must handle:

- anchors and aliases;
- duplicate keys, reported as warnings with last-one-wins;
- multi-document files;
- line and column positions.

JSON and `.properties` are parsed as well.

**`flatten`.** Follows the rules in `docs/domain-model.md`. Placeholders are kept literally, and `yes`/`no`/`on`/`off` values are flagged.

**Hard limits** on nesting depth, alias expansion (to block alias bombs), key count and scalar size. When a limit is exceeded, return an error, never a panic or a hang.

**Verify:**

- golden tests;
- fuzz targets for YAML, JSON and `.properties`, including alias-bomb seeds;
- a benchmark: parse and flatten of the 6,000-line file in 20 ms or less.

### T3: Settings index and locations

**`index_settings(blob)`** returns rows of setting path, value text, type, start line and end line.

**Locations** come from:

- tree-sitter with the YAML grammar, for YAML;
- parser positions, for JSON;
- line numbers, for `.properties`.

**`locate(file, setting_path)`** returns a line range.

**Verify:** for every setting in the fixtures, the tree-sitter location agrees with the parser. The opt-in real-snapshot run checks the same.

### T4: Paths and merge modes

- `map_path` and `reverse_map` through `PathMappingRule`.
- `globset` for all glob rules.
- Classification into base, tenant or untracked, with the logical file and branch.
- **`effective`** for both merge modes:
  - `whole_file`;
  - `spring_merge`: maps merge, while scalars and lists are replaced. Root `application.*` files are an optional lowest layer.
  - Every effective value records where it came from.
- **`effective_tree_hash`** per directory.

**Verify:** golden tests for both modes, and property tests that tree hashes change exactly when any effective file changes.

### T5: Drift, diffs and grid

- **`drift_state`** follows the table in `docs/domain-model.md`. Line endings are normalised for `structured` and `text` classes.
- **Diffs:**
  - text diff with the similar crate, using a patience or histogram algorithm;
  - line-ending-only differences are hidden but flagged;
  - output is hunks ready for a virtualised view;
  - a semantic diff for structured files;
  - binary compare by hash and size.
- **`grid`** over effective configs, with a channel filter.

**Verify:**

- table tests for every drift row;
- a benchmark: compare of a 6,000-line file in 100 ms or less;
- a benchmark: grid for one logical file across 40 swimlanes in 150 ms or less, using pre-flattened inputs.

### T6: Checks C1–C8 and effect previews

**Checks.** Implement each check exactly as defined in `docs/domain-model.md`:

- C1 handles both merge modes.
- C6 classifies hosts, excluding cluster-local names (single labels without a dot).
- C8 reports duplicate keys.

**Effect previews:**

- deleting a tenant file shows the fallback to the base file, with the semantic diff;
- uploading a new tenant file warns that it forks from base;
- a suffix that doesn't match the deployed branch means the file is never read;
- changing a root file affects every service.

**Verify:** golden tests. In the opt-in real-snapshot run, sit1 must report:

- C2 for `account-sorting-config.yml`, `miniStatementConfig.yml` and `receipt-ci_1.bmp`;
- C1 for the missing `atm` entry in `entryGroupConfig.yml`;
- C8 for the three keys in `security-roles.yml`.

### T7: Secret scan and text search (S11, S13)

**`secret_scan`** is value-based:

- private key headers;
- known token prefixes;
- JWT-shaped strings;
- strings that look random, above a length threshold.

It never flags on key names alone or on placeholders. It returns line numbers and lengths, never values.

**`search_text`:**

- built on `grep-regex` and `grep-searcher` over in-memory blobs;
- literal by default; regexes are compiled with size limits;
- case option, total and per-file caps;
- binary files skipped;
- results carry line text without `\r`, and the match ranges.

**Verify:**

- fuzz the regex options;
- a benchmark: text search over the target-scale deduplicated blob set, running on rayon, in 400 ms or less.

### T8: Batch functions and determinism (P5, P6)

**Batch variants** (findings for many swimlanes, adoption ranking, full recompute) use rayon internally. Document that callers must run them from a blocking context.

**Determinism.** Stable sorting everywhere, and no unordered-map iteration in any output.

**Verify:**

- a P6 benchmark of a full recompute at target scale, in 3 minutes or less on 2 vCPU;
- a P5 benchmark of an incremental recompute;
- a test that runs everything twice and compares the output byte for byte.

### T9: Severity and inline ranges (D76, D81)

- **`severity(context, rules)`** applies the default rules in `docs/domain-model.md`, plus admin-configured ones. It returns the severity and the ids of the matching rules, deterministically.
- **Inline ranges:** text diff hunks include the changed character ranges within each line, from `similar`'s inline changes. Word boundaries are tuned for YAML keys and values.

**Verify:**

- golden tests for each default rule, and a property test that adding a rule never lowers a severity;
- inline-range golden tests;
- the diff benchmark still meets 100 ms for 6,000 lines.

### T10: Config-server resolution model (D82–D84, D87)

Replace the glob-based merge-mode rules with the resolution model in `docs/domain-model.md`.

- **`file_role(path, application_names)`** classifies a file as a property source or a resource.
- **`property_view(app, tenant, files)`** merges with the documented precedence. A test fixture mirrors the config-server's own test data: `application.properties` with `application-bis.properties` must yield `CORE_ROUTING=true` and `LOG_LEVEL=DEBUG`.
- **`resolve_resource(file, tenant, channel, files)`** chooses the whole file: the tenant file, then the channel folder.
- **`render(resource, property_view)`** substitutes `${KEY}` only when the key exists. Its output must equal the config-server's expected files: `tx-infinity-core-resolved-bis.yml` and `entryGroupConfig-resolved-bis.yml`, ported as golden fixtures.
- **Search locations** are computed from `channels.yml` and the directory tree.
- **New checks:** C9, C10 (taking environment-variable names as input) and C11.
- **Pickup state** per change and service, from the file role, the client settings and the config-server's start time.
- **Effect previews gain:**
  - "property source: refreshes every client with notifications enabled, or needs a restart";
  - "new folder: the config-server must restart before it serves files here";
  - "`channels.yml` change: the config-server must restart".

**Verify:**

- the ported config-server fixtures match byte for byte, after line endings are normalised;
- on the real base snapshot, C9 reports the 13 ambiguous names;
- property tests on precedence ordering.

## Acceptance (all required)

- [ ] `just verify-04` passes, and every engine budget is met in `bench-check`.
- [ ] Fuzz targets exist for the YAML, JSON and `.properties` handling, flattening, path mapping and regex options, and `just fuzz-smoke` runs clean.
- [ ] Output is deterministic.
- [ ] The opt-in real-snapshot expectations are met.

## Stop and escalate if

- No YAML crate meets both the correctness requirements (anchors, duplicates, positions) and the 20 ms budget.
- The tree-sitter YAML grammar disagrees with the parser on real files, and you can't reconcile them.

## Out of scope

All I/O, and the decision about when to recompute. That belongs to 05.
