# Threat model (STRIDE)

Each prompt fills in the section for the components it owns, and keeps it current. A human signs off before go-live (S24).

For each component, record:

- **Assets.**
- **Trust boundaries.**
- **STRIDE threats:** Spoofing, Tampering, Repudiation, Information disclosure, Denial of service and Elevation of privilege. Give each threat its mitigation, with the S# it maps to.
- **Residual risks.**

## Components

- Agent (02)
- Hub identity and audit (03a)
- Hub platform: gateway, routing, events, Git, webhooks (03b)
- Engine (04)
- Registry and read API (05)
- Write paths and token vault (06)
- MCP server (07)
- Web app (08)
- Deployment and supply chain (09)
- Docs search (14)
- NFS VM sentinel (17)

## Contracts, ports, migrations, fixtures and fuzz skeleton (01)

Owner: prompt 01. This section covers what 01 creates and everything else builds on: the database roles and the audit table protections (`db/migrations/0001_init.sql`, `deploy/dev/init.sql`), the `Secret<T>` type, the typed validators (`NfsPath`, `RepoPath`, `CompareRef`, `GitRef`, `TenantId`, `PathMappingRule`), the proto and OpenAPI contracts, the synthetic fixture generator and the fuzz skeleton. Behaviour behind the ports belongs to the prompts that implement them and is covered in their sections.

### Assets

- The audit chain (`audit_events`, `audit_checkpoints`): the evidence of who changed what (S9).
- Secrets in memory: GitHub tokens, session ids, join tokens, webhook URLs, keys (S7, S8, S21).
- The NFS tree, which only typed paths may address (S11, S17).
- The two database roles, `lanekeeper_migrator` (owns the schema, runs DDL) and `lanekeeper_app` (the hub's runtime identity).
- The contract files themselves, which every other prompt trusts.

### Trust boundaries

- Hub process to PostgreSQL: the hub connects as `lanekeeper_app` and never as the migrator or a superuser.
- Network or API input to the typed validators: the first parse of every path, compare reference, tenant id and swimlane id.
- Agent and sentinel to hub over gRPC: message size limits and the join request shape live in `proto/agent.proto` and `crates/proto`.
- Developer machine and CI to the repository: fixtures are generated, never copied from a real environment.

### STRIDE

| | Threat | Mitigation | S# |
|---|---|---|---|
| S | An agent presents itself as a sentinel, or the reverse, to obtain the other identity. | `JoinRequest` carries `PeerKind`; the hub issues a certificate SAN that includes the kind and authorises RPCs by kind. `UNSPECIFIED` is rejected. No agent-facing join-token endpoint exists in `api/openapi.yaml` (`no_agent_join_token_endpoint`). | S5, S25 |
| S | A user acts as someone else through a forged or replayed browser session. | Every non-GET operation in the OpenAPI contract requires CSRF and `Idempotency-Key`; every operation declares `x-required-role`; the route-guard harness (`ports::conformance::route_guard`) fails any route without a matching guard. Implemented by 03a. | S3, S4 |
| T | The application role, or code running as it, edits or deletes audit rows. | `lanekeeper_app` holds only INSERT and SELECT on `audit_events` and `audit_checkpoints` (explicit per-table grants, no default privileges). A trigger refuses UPDATE, DELETE and TRUNCATE for every role, the owner and superusers included, and is `ENABLE ALWAYS`, so `session_replication_role = replica` does not bypass it. Tests: `app_cannot_update_audit_events`, `app_cannot_delete_audit_events`, `app_cannot_truncate_audit_events`, `trigger_blocks_update_and_delete_even_for_owner`, `app_has_only_insert_select_on_audit_events`. | S9 |
| T | The audit chain is forked or a row is forged with a wrong hash. | `UNIQUE(prev_hash)` stops a fork; a check constraint requires `hash = sha256(prev_hash \|\| event_json)`; the canonical JSON that was hashed is stored. Tests: `audit_prev_hash_cannot_fork`. | S9 |
| T | The runtime role changes the schema, for example to add a trigger or grant itself rights. | `lanekeeper_app` owns nothing and has no DDL right (`app_cannot_run_ddl`). Roles are created outside the migration (`deploy/dev/init.sql`, CloudNativePG managed roles) and the migration refuses to run as any other role. Every table needs a recorded grant decision (`every_table_has_a_grant_decision`), so a new table cannot silently inherit broad rights. | S9, S21 |
| T | A path escapes the NFS root (`..`, absolute path, backslash, NUL, control character, over-long path). | `NfsPath` is the only path type that reaches the agent or sentinel. It rejects every traversal form, NUL and control characters, backslashes and absolute paths, normalises `//` and `.`, and bounds the length (1,024 bytes; 255 per component). Fuzz target `nfs_path` asserts the invariants on arbitrary input, and its seed corpus replays on stable in `corpus_replay.rs`. The agent's cap-std root (02) is the second layer. | S11, S17, S22 |
| T | A tenant suffix is split wrongly, so a write lands on the wrong file. | `PathMappingRule::reverse` strips only an exact `-<tenant>` for a known tenant (the longest wins) and never splits on the first hyphen. Fuzz target `path_mapping_reverse` asserts that forward mapping of the reverse result gives the original path. | S11, S22 |
| T | A compare reference smuggles a path, option or ref syntax (`..`, `@{`, a leading `-`) into a Git call. | `CompareRef` and `GitRef` follow `git check-ref-format` rules, are bounded, and are rejected rather than repaired. Fuzz target `compare_ref` asserts a display and parse round trip. | S11, S22 |
| T | A tampered dependency or lockfile changes behaviour. | Exact version pins at least 14 days old, a committed `Cargo.lock`, `cargo deny` and `cargo audit` in `just verify`, and a separate committed lockfile for the fuzz crate, checked by `just fuzz-smoke`. | S19 |
| R | A state change leaves no trace, or the trace is rewritten afterwards. | Audit events are written in the same transaction as the change (`AuditLog::record(&mut tx, ..)`). The chain is append-only and hash-linked as above; 03a adds hourly KMS-signed checkpoints in a locked-retention bucket, so a rewrite is detectable. | S9 |
| I | A secret reaches a log, an error, an audit event or a response. | `Secret<T>` prints `[redacted]` in `Debug`, has no `Display`, no `Serialize` and no `Clone`, compares only in constant time (`ct_eq`) and is zeroized on drop; `Deserialize` exists for `Secret<String>` only, for loading configuration. Tests: `debug_is_redacted`, `zeroize_on_drop`, and static assertions that `Secret` is not `Serialize`, `Display`, `Clone` or `PartialEq` (`crates/domain/tests/secret.rs`). The generated `Debug` of `JoinRequest` is replaced by a redacting one (`join_tokens_never_reach_debug_output`). Validator and port error enums never carry the rejected input (`errors_do_not_leak_inputs`). | S7, S21 |
| I | Real tenant configuration leaks through test data. | `cargo xtask gen-fixtures` builds everything from a seed with invented names and content; output goes under `target/fixtures/`, which is git-ignored; nothing real is read or committed. Two runs with the same seed give identical output, so the data is reproducible and reviewable. | AGENTS rule 12 |
| I | The application role reads more than it needs. | Per-table grants: append-only, write-once and mutable tables are distinct decisions; no TRUNCATE, REFERENCES or TRIGGER is ever granted; `_sqlx_migrations` is not readable by `lanekeeper_app`. | S9, S21 |
| D | Oversized or unbounded input exhausts memory. | gRPC messages are capped at 4 MiB after decompression, a `ScanDelta` at 3 MiB and a fixed entry count (`proto::limits`); OpenAPI lists use keyset pagination with a `limit` of at most 500; MCP inputs and outputs are bounded (`docs/mcp-tools.md`, lint in `mcp_tools_doc.rs`); `TenantSet` is capped; every path and ref has a length limit. | S11, S14 |
| D | The sentinel partition maintenance blocks inserts or fills the disk. | A default partition means an insert never fails for want of a partition. `maintain_sentinel_partitions` is idempotent, serialised by an advisory lock and validates `keep_days` (1 to 3650). | S9 |
| E | The one `SECURITY DEFINER` function is used to run arbitrary DDL. | `maintain_sentinel_partitions(integer)` is owned by the migrator with a pinned `search_path` (`pg_catalog, pg_temp`), schema-qualified names and format-quoted identifiers; it takes only an integer; `EXECUTE` is revoked from PUBLIC and granted to `lanekeeper_app` alone. It can only create and drop partitions named `sentinel_records_yYYYYmMM` of that one table. Test: `sentinel_partition_function_is_the_only_ddl_path`. | S9, S21 |
| E | A proposal's author approves their own change. | A trigger on `approvals` refuses an approval by the proposal's author, independent of application code. | S4 |
| E | A route or MCP tool is added without a role. | Every OpenAPI operation needs a valid `x-required-role`; the route-guard harness and the MCP doc lint (role floor against OpenAPI, confirmation on write tools) fail otherwise. | S4, S14 |

### Residual risks

- **Owner-level database access.** The migrator owns the tables, so it can disable or drop the audit triggers. Mitigation is outside this prompt: the migrator credential is used only by the migration job, not by the hub, and the signed checkpoints (03a, S9) let a nightly job detect a rewrite. A rewrite after the latest checkpoint, at most one hour, is detectable only by the Cloud Logging copy.
- **The definer function is a privileged surface.** It is small and bounded, but anything that can call it as `lanekeeper_app` can drop sentinel partitions older than the retention window. The retention window is data already past its 90-day life; the audit tables are not touched.
- **`Secret::expose` is an escape hatch.** The type stops accidental formatting and serialisation, not a caller that reads the plaintext and logs it. Reviewers grep for `expose()`, and the log-capture tests of later prompts cover the paths that matter.
- **Rejection of unusual file names.** `NfsPath` rejects backslashes and control characters, so a real file with such a name cannot be addressed. This is deliberate (plan Q13); it surfaces as a finding rather than a silent skip.
- **A base file whose name ends in a known tenant id** is reported as a tenant file (plan Q14). This is a limit of the naming scheme, documented in `mapping.rs`.
- **Fuzzing coverage is a skeleton.** Three targets run 30 seconds each per smoke run. Parsers added by later prompts (YAML, `.properties`, Markdown chunking, webhook payloads, proto decoding at the hub) must add their own targets (S22).
- **The development database passwords** in `deploy/dev/init.sql` are for local use only; production roles come from CloudNativePG managed roles (09).

## Known residual risks (from design)

- **Partly attributed NFS changes.** With the sentinel (D72), edits made on the VM are attributed to a named OS Login user. Writes from NFS clients that happen outside a sync Job are attributed only to "an NFS client", at medium confidence. A root user on the VM could stop auditd; the sentinel's heartbeat gaps are flagged as findings.
- **Cursor's model providers.** Config content sent through Cursor reaches Cursor's model providers. This is accepted because the files contain no secrets (D18), and the six flagged files are pending review (Q20).
- **Delayed lockout.** A user disabled in Entra keeps any active session until it is ended in the app or expires, which takes at most 12 hours. Mitigation: admins can disable the user in the app, which ends their sessions immediately.
