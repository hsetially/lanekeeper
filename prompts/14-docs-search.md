# 14: Docs search, docs filesystem and feature status

| | |
|---|---|
| **Wave** | 2. Can start against fakes. |
| **Depends on** | 01. Integrates with 03b (Git, blobs, events), 04 and 05. |
| **You own** | `crates/hub-docsearch/**`, the docs section of `docs/threat-model.md` |
| **Interfaces** | Implements `DocSearch`. Consumes `GitReader`, `BlobStore`, `EventBus`, `AuditLog`, `Users`, `RegistryRead` and the engine. |
| **Security** | S4, S9, S10, S11, S12, S15, S21, S22 |
| **Performance** | P7 (docs search 150 ms, feature status 200 ms), P9 |
| **Gate** | `just verify-14` |

## Objective

Make the configuration docs fast to search, exactly greppable and safe to render. Tie them to real config values through the feature-status view.

## What the real docs look like

- 32 Markdown files, mostly with CRLF line endings.
- Each one has the same sections:
  - an H1 title;
  - Related Files: backticked paths relative to `data/config/`;
  - Overview;
  - Feature Flags: a table with the columns Flag, File, Channel, Template default and Description;
  - Configuration Locations;
  - To Enable and To Disable.
- A README indexes the others.

## Tasks

### T1: Sources

**Git:** configured as (repo, branch, path glob). The default is `templates/common-docs` with `**/*.md`.

- Re-index on `GitHeadMoved`, processing only the changed paths, which come from the tree diff.
- These docs are read-only in the UI.

**Uploads:**

- Editors and above; UTF-8 Markdown only, up to 2 MiB.
- Originals go to the CMEK-encrypted GCS bucket.
- Uploading a file with the same name creates a new version. Deletion is soft.
- Every change is audited.

**Verify:** incremental re-index tests, and tests that rejected uploads (wrong type, wrong encoding, too large) never reach storage.

### T2: Chunking, extraction and rendering (S12)

**Chunking.** Parse with pulldown-cmark after normalising line endings. Split at H2, carrying the H1 title and the heading path. Split long sections at H3. Never split a table or a code block.

**Extraction:**

- related files: backticked paths, normalised relative to the config root;
- setting paths: backticked dotted keys;
- the Feature Flags tables, into `documented_flags`. Each flag is resolved to a setting-path pattern with an indexed query on `settings_index` for the named file and channel. Unresolved flags are listed for admins.

**Rendering.** Render HTML on the server, sanitised with ammonia, using an allowlist of tags with no scripts, styles, event handlers or `javascript:` URLs. The web app displays only this sanitised HTML.

**Verify:**

- golden tests for chunking;
- an XSS test corpus that must come out sanitised;
- a fuzz target for the chunker.

### T3: Embeddings and hybrid search (P7)

**Embeddings.** A pinned 384-dimension model, run in-process through ONNX on CPU and loaded once. Inputs are batched. Record the model and its revision in `docs/decisions.md`.

**Search:**

1. Run two searches in parallel:
   - Postgres full-text search (`websearch_to_tsquery`, ranked with `ts_rank_cd`);
   - pgvector HNSW cosine search.
2. Fuse the two result lists with reciprocal rank fusion.
3. Boost exact matches of identifiers, such as flag names, paths and setting paths.

**Caching.** Query embeddings are cached in moka.

**Verify:** a benchmark showing p95 of 150 ms or less on the full set with a warm model. On the real docs (opt-in), "account sorting" ranks `account-sort.md` first.

### T4: Docs filesystem (D57)

**Virtual tree:** `/git/<branch>/<path>` and `/uploads/<name>`.

**`read(path, lines?)`** returns the doc text, with an optional line range.

**`grep(pattern, regex?, context)`:**

- uses `grep-regex` and `grep-searcher` over the stored text;
- literal by default; regexes are compiled with size limits;
- results are capped.

There is no shell and no real filesystem.

**Verify:** tests and a fuzz target for path handling. On the opt-in real run, `grep("enableAccountSorting")` returns `account-sort.md` with context.

### T5: Docs for a file, and feature status

- **Docs for a file:** chunks whose related files include the file's logical path, served from an index table.
- **Feature status:** for each documented flag, swimlane and channel, the effective value, the template default and whether they differ. Built from settings-index joins, with no file parsing at request time.
- **Live updates:** feature status is refreshed when `DriftChanged` arrives.

**Verify:**

- a feature-status benchmark of 200 ms or less at target scale;
- on the opt-in real run, `enableAccountSorting` resolves for `atm-iso` and `remote-itm-teller`.

### T6: Threat model

Fill in the docs section of `docs/threat-model.md`.

## Acceptance (all required)

- [ ] `just verify-14` passes, and the docs budgets are within limits.
- [ ] The XSS corpus is fully sanitised, and the upload validation tests pass.
- [ ] Viewers can search; uploads require Editor; Git-sourced docs can't be deleted in the UI.

## Stop and escalate if

- No 384-dimension model meets the quality bar on the real docs. A different dimension would need a contract change.

## Out of scope

Chat or generated answers. Cursor writes the answers.
