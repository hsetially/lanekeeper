# contract/08-web: commit 13 (masking, CG7 and M1) is blocked

Written by the implementer of commits 13 and 14. Branch `contract/08-web`, HEAD `2147fbe` when the work stopped. Nothing of commit 13 or 14 is committed. The branch is exactly as the integrator left it, plus this file.

## The problem

The plan says commit 13 touches four files and that `docs/mcp-tools.md` is never touched (F5: "no tool needs a field"). Two pinned tests of `crates/ports/tests/openapi_conventions.rs` tie the REST document to something outside the allowed list. M1 as approved fails both.

Measured with the full work of commit 13 in the tree (`cargo test -p ports --all-features --no-fail-fast`: 2 failures, every other test passes):

1. `mcp_write_definitions_match_the_rest_document` (line 1266). The test says "an assistant and the web app must see one set of rejection reasons, issue kinds and line ending styles". It compares `ValidationIssue.kind` of `api/openapi.yaml` with `$defs.ValidationIssue.properties.kind.enum` of `docs/mcp-tools.md:232`.
   - Left (MCP document): `duplicate_key`, `parse_error`, `secret_match`. Right (REST, with the new kind): the same three and `redaction_marker`.
   - `docs/mcp-tools.md` is on the never-touched list. `crates/hub-mcp/src/schemas/tools.json` is generated from it (7 copies of that enum, `contract_schemas::embedded_schemas_equal_the_contract_document`), and is not on the allowed list either.
   - The MCP side needs the value anyway. The main case of the marker refusal is a Cursor agent that reads `[REDACTED: N chars, rule]` through an MCP tool and writes it back: that write is refused with `issues[].kind = redaction_marker`, and the MCP error schema would reject the value. `maxLength: 16` on the MCP `kind` fits `redaction_marker` exactly (16 characters).
2. `served_file_is_the_domains_and_the_tool_adds_redaction_fields` (line 1557). REST `ServedFile` must have exactly the domain `ServedFile`'s wire fields, all required, and the MCP tool `get_served_config` must have those plus three. M1 puts `masks` at the REST edge (the plan says domain: `IssueKind` only), so `masks` is a REST property with no domain field, and it is optional.

## What I verified, so that the decision costs nothing more

- Resolution R1 below, applied in the tree and reverted: `cargo test -p ports --all-features --no-fail-fast` 259 passed, 0 failed. The patch is saved (see "Saved work").
- `cargo check --workspace --all-targets --locked` is clean with the new `IssueKind` variant: no exhaustive match on `IssueKind` exists in any crate (all uses construct, compare or print `as_str()`).
- `redocly lint` valid, the additive check 0 findings, `cargo fmt --check` and `cargo clippy -p domain -p ports --all-targets -D warnings` clean, `cargo test -p domain` all pass.
- Not run: `cargo test -p hub-mcp` (the regeneration of `tools.json`, heavy build), `just`, Docker-backed tests, `cargo deny`, `cargo audit`.

## Options (the human decides)

- **R1 (recommended, the smallest change that keeps M1 whole).** Three extra touches, each a decision of yours:
  1. `docs/mcp-tools.md`: add `"redaction_marker"` to the one enum at line 232 (nothing else).
  2. `crates/hub-mcp/src/schemas/tools.json`: regenerate (`UPDATE_SCHEMAS=1 cargo test -p hub-mcp --test mcp_auth contract_schemas::embedded_schemas_equal_the_contract_document`); the diff is the enum value in 7 places. Unverified here.
  3. `crates/ports/tests/openapi_conventions.rs`: the `ServedFile` pin keeps its exact-set equality but excludes the one edge-only property `masks` (required stays "all of the domain's fields"). Inside Check 1's pattern, not named in the plan. Not a weakening: the set is still exact.
- **R2.** Drop `redaction_marker` from commit 13 (no issue kind; 06 would refuse with `invalid_content` and no issue). Loses the machine-readable cause for the web and MCP.
- **R3.** Drop `masks` from `ServedFile` (two carriers, not three). The served view then relies on the marker pattern only, with no reveal and no locator.
- **R4.** Give the domain `ServedFile` a `masks` field. Touches the 10 `ServedFile` literals (in `domain`, `ports` and `hub-mcp` tests and one bench), and contradicts "masking at the edge, never in a port".

## Saved work (scratchpad, `/tmp/claude-0/-home-user-lanekeeper/44c626c4-c7b9-55f1-aa0f-edfeb3376d24/scratchpad/cg13-wip/`)

- `cg13-work.patch`: `git diff` of `api/openapi.yaml`, `crates/domain/src/enums.rs`, `crates/domain/tests/web_additions.rs`, `crates/ports/tests/web_contract_global.rs`.
- `web_contract_masking.rs`: the new test file (12 tests, 6 of them teeth tests).
- `proposed-r1-pins.patch`: R1 items 1 and 3.

Test-first evidence: before the document and the enum changed, 9 of the 12 masking tests and `redaction_marker_is_a_kind_on_the_wire` (domain) failed for the right reason (`schema Mask is not in the document`, `IssueKind has no redaction_marker`). Mutation checks on the real document each failed the intended tests: drop `FileContent.masks`, drop `redaction_marker` from the OpenAPI enum, add a `value` field to `Mask`, drop the statement of `DocHit.snippet`, make `masks` required on `DocText`, and (domain) remove the variant (E0599 in three tests).

## What the work of commit 13 contains, and where it differs from the plan

- `Mask {line, start, end, setting?}`: `start` and `end` are half-open UTF-8 byte offsets into the `text` of the same object (`text[start..end]` is the marker), `line` is 1-based in that text. This is the shape of the brief and of plan 08 ("masks: [{line, start, end, setting?}]"), not the table text of CG7 in the plan (`column`, `length`, `hidden`, `rule`, in characters). `InlineRange`'s description now says that the secret scan counts characters and the server converts.
- `IssueKind` lives in `crates/domain/src/enums.rs:109`, not `write.rs`. It is inside Check 1's pattern, not in the plan's row for commit 13.
- `getBlob` is a bypass of M1 that the plan does not mention: it serves a file's own bytes by hash, immutable. The work adds one sentence: it never serves a file whose text the secret scan flags (404), the masked text being `getFileContent`'s.
- The marker pattern allows digits in the rule name (`[a-z0-9_]+`, a superset of the plan's `[a-z_]+`, so that a later rule cannot slip past the refusal).
- `crates/ports/tests/web_contract_global.rs` is extended for `Mask` and the three `masks` properties (it is inside Check 1's pattern as a new file of commit 12).
- Not in the plan, for the human to weigh: text search matches against the stored text, so `searchText` and `searchSettings` answer whether a hidden value contains a given string even though every hit's text is masked (an oracle for a flagged value). M1 as planned does not close it; the owners of 04 and 05 decide how search treats flagged values.

## After the decision

Apply `cg13-work.patch` and the chosen resolution, rerun the gate, commit 13, then commit 14 (docs). `docs/interfaces.md` will say, for R1, that 06 and 07 own the marker refusal and the MCP document follows.

Check 1 will list this file (`plans/contract-08-web.blockers.md`); the plan's pattern allows only `plans/contract-08-web.md`.
