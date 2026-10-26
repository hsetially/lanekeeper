# 18: Hub assembly

| | |
|---|---|
| **Wave** | 3 |
| **Depends on** | 01, 03a, 03b, 05, 06 and 07, all merged. Prompt 14 is not needed: a `DocSearch` that says it is unavailable stands in until it lands. |
| **You own** | `crates/hub/**`, the hub-assembly section of `docs/threat-model.md`, `docs/hub-assembly-survey.md`. `Cargo.toml` (the workspace-dependency lines) and `Justfile` (the `verify-18` recipe) are shared files. Every other crate is off limits. |
| **Interfaces** | Composes every port and every module. Changes no port, no migration and no OpenAPI operation unless the plan lists a `Contract-change:` and a human approves it. |
| **Security** | S1, S2, S3, S4, S5, S6, S7, S9, S10, S12, S16, S18, S21 |
| **Performance** | P11 (startup under 10 seconds, memory), and no regression of P2, P12 and P13 through the wiring |
| **Gate** | `just verify-18` |

## Objective

Make `hub` a real server. Today `crates/hub/src/main.rs` reads its settings and exits 1, because nothing links the identity module, the
registry, the write paths or the MCP server in, and no listener, migration, signal handler or job is started. Every part exists in
its own crate and is tested there; **nobody has been given the job of putting them together** (`docs/hub-assembly-survey.md`;
prompt 09's plan A1 and A8; D109, D112).

When this prompt is done:

- `hub` starts against a real Postgres, applies nothing it should not, and serves the web app, the API, the live stream, the MCP
  server, the Git webhook and the blob endpoint on the web listener, and the agent and sentinel gRPC service and the replica
  service on their own mTLS listeners;
- it refuses to start, and says why, whenever a real component is missing. It never falls back to a fake, because a fake session store
  in production would let anyone in;
- every route of every module has a guard, every state-changing route is protected against CSRF, and one test proves it over the
  whole OpenAPI document;
- agents and the NFS sentinel can join and send reports, and the swimlanes they belong to can be registered by a declarative,
  audited command;
- prompt 09's kind test can go green, and `just dev` runs a working hub.

**Read first:** `docs/hub-assembly-survey.md` (what exists, what is missing, the startup sequence the code implies), then
`plans/03b-hub-platform.evidence.md` and `plans/05-registry-read-api.evidence.md` (the "needs-human" and "follow-ups the hub must
wire" sections), `plans/06-write-paths.pr.md`, `plans/07-mcp-server.pr.md`, `crates/hub/HARDENING.md`, and
`crates/hub/benches/startup.rs` (`child()`) and `crates/hub-platform/tests/support/cluster.rs`, which are the nearest things to a
composition. The survey was written by an agent that ran no code: re-verify what you rely on.

## Decisions at the plan gate

The plan lists each of these as an open question with the proposed default below. A human answers them when approving the plan.
Do not start the task that depends on one before the answer is in the approved plan.

| # | Question | Proposed default |
|---|---|---|
| 1 | **Which `GuardedRouter` survives, and where CSRF lives.** `hub_platform::GuardedRouter` (no CSRF, authenticates through `SessionAuth`) and `hub_identity::GuardedRouter` (CSRF, `AuthLayer`, its own default-deny) are different types and `hub-identity` cannot implement `SessionAuth`. 03b evidence #1 and #5; D109. | Keep the platform guard for every module. In `hub`, implement `SessionAuth` over `hub_identity::Sessions` (a cookie-session adapter), mount identity's routes through an adapter, and apply **one** CSRF layer (`CsrfLayer`, after authentication) to every non-GET route of every module, so no module can forget it. If identity's router cannot be merged into the hub's, nest it as a self-contained sub-router and run its own route-table test. The plan shows what it read and why. |
| 2 | **Per-user `SessionAuth::still_valid` against per-cookie sessions.** A logout does not end that browser's SSE stream. | End the stream on the session-revocation event for that session id, using `RevocationListener`; if that needs a `hub-platform` change, list it as an Extra-path. |
| 3 | **The web listener's certificate.** HARDENING.md says HTTPS behind the users' load balancer; no setting exists. | A certificate chain and key read from two Secret Manager secrets named in the settings, kept in memory, reloaded on a timer. Never a Kubernetes Secret, never a variable. |
| 4 | **Migrations.** Nothing runs them in production; the image carries only the binary and `web/dist`; `0001` insists on `lanekeeper_migrator`. 09 plan A8. | A subcommand, `hub migrate`, with the migrations embedded at build time, run as a Job with the migrator credential (a file) before the hub rolls. **The hub never migrates at start.** |
| 5 | **`DocSearch`.** `McpDeps.docs` is required and prompt 14 is not built. | An explicit `Unavailable` implementation: every call returns a typed "docs search is not available", and `/readyz` and the logs say so. No fake data. |
| 6 | **The 33 admin operations and 17 other unrouted operations (D112).** | Out of scope, listed in the plan as an allowance with the owner "unassigned". The route-table test names each as an explicit spec-only allowance, so a new route cannot hide among them. |
| 7 | **Job wrappers for `hub-writes`' workers** (`ReconcileWrites`, `ExpireProposals`, `PrJobRunner`, `CredentialCleanup` are bare `run_once` methods). | Wrappers in `crates/hub` implementing `Job`, with lease names `writes-reconcile`, `writes-expire-proposals`, `writes-pr-jobs`, `writes-credential-cleanup`; intervals and ttls from D63 and the doc comments, in settings with those defaults. |
| 8 | **The body cap.** `LK_BODY_MAX_BYTES` defaults to 4 MiB; `hub-writes` accepts edit bodies up to 12 MiB + 64 KiB. | A per-route limit: the edit and upload routes take the cap their module declares; every other route keeps the global one. |
| 9 | **Settings schema.** Nothing outside `crates/hub` reads the environment; secrets are files or Secret Manager names; `hardening.rs` rejects secret-looking variable names and requires every `LK_*` of HARDENING.md in `config.rs`. | Group the settings (listeners, TLS and CA, GCP, Entra, identity, registry, writes, MCP, jobs). Secrets are `*_FILE` paths or Secret Manager secret **names**; typed errors name the variable and never the value. HARDENING.md and `hardening.rs` change in the same commit. |
| 10 | **CA material.** Two KMS keys and two CA certificates (plan A7), the replica's stable name and advertised address, the hub audience. | Key refs, certificate secret names, `LK_REPLICA_NAME` (the StatefulSet pod name), `LK_REPLICA_ADDRESS` and `LK_HUB_AUDIENCE` as settings; certificates are public and come from Secret Manager. `CaIssuer::new` runs at start and the hub refuses to start on a KMS mismatch. |
| 11 | **Readiness.** What gates `/readyz`, against P11's 10 seconds. | Postgres, mirrors initialised, both CA probes passed, the event-bus listener connected and the job runner started. Registry warm-up does not gate it. |
| 12 | **MCP.** OAuth against personal tokens, session affinity across three replicas (07 evidence), the notify-services switch. | Both credential modes as 07 built them, chosen by `LK_MCP_AUTH`; the `Mcp-Session-Id` affinity is reported to prompt 09 as a chart requirement, not solved here; `notify_services` off unless set. |
| 13 | **Unbuilt callers:** `SyncService::record_sync` (no caller), adoption and "mark intentional" routes, `GET /api/v1/proposals/{id}`, `/drafts/{id}`, `/prs/preview`, the two restart-offer routes. | Out of scope, listed in the plan as spec-only allowances, except `record_sync`: wire its caller if 05's evidence names one, otherwise report it. |
| 14 | **One metrics registry, one clock, one HTTPS client.** | One `Metrics` registry through `Metrics::register`; one `SystemClock` adapted to each crate's `Clock`; keep the two HTTPS stacks (no change to another crate) and record the duplication as a residual. |

## Tasks

### T1: The process (S6, S7, S16)

- A Tokio multi-thread runtime in `main`; settings parsed and validated first (exit 2 with a typed message, no values), then
  telemetry, then signal handlers installed **before** the first poll (the 03b blockers file records a SIGTERM race).
- The settings schema of decision 9, with `HARDENING.md` and `crates/hub/tests/hardening.rs` updated together (the new variables,
  volumes if any, and `storage.googleapis.com` in the egress list).
- The web listener with TLS from decision 3, and the ops listener (`/healthz`, `/readyz`, `/metrics`).
- `Drain` bridged to the `closing` channels that the platform services take; shutdown order: not ready, wait the drain period, stop the
  accept loops, stop the job runner, wait for the listeners, flush telemetry, exit 0. A second signal exits at once.
- Readiness per decision 11.

**Verify:** unit tests for every setting (missing, malformed, secret-looking names refused); a process test against a real Postgres
that starts the binary, gets `/healthz` and `/readyz`, sends SIGTERM and sees a clean exit within the grace period.

### T2: Migrations (S10)

- `hub migrate` (decision 4): embedded migrations, the migrator credential from a file, `verify-full` TLS like the app pool,
  idempotent, refuses to run as `lanekeeper_app`, prints the versions applied and never a credential.
- Document how to run it in `crates/hub/README.md`; prompt 09 will add the Job.

**Verify:** against a real Postgres: an empty database ends with every migration applied; a second run changes nothing; `lanekeeper_app`
is refused; a database with a later migration than the binary knows is refused with a typed error.

### T3: Platform wiring (S5, S6, S7)

Wire everything `hub-platform` provides, in the order of the survey's startup sequence: the Google clients, Secret Manager, the KMS
signer and envelope, `PgAuditLog`, `PgLeases` with this replica's name, `PgEventBus` and its listener, the Git mirror, fetch worker,
syncer and webhook, `PgBlobStore` with `RegistryBlobPolicy` (T5), both `CaIssuer`s, the server identities and their rotation, the
agent listener (8444: `JoinService`, `AgentService` with the gateway, `SentinelService`) and the replica listener (8445:
`ReplicaService`, `ConnectionRegistry`, `HubGateway`, liveness), and the `JobRunner` with the four platform jobs.

- No fake outside `#[cfg(test)]`; `release_build.rs` keeps passing. If a real component cannot be built from the settings, the hub
  refuses to start and names it.
- Every task spawned here is owned: it stops on the `closing` signal, and a panic or error in one is logged and turns readiness off
  where the component is required.

**Verify:** an integration test in the style of `cluster.rs` but through the real composition code: two replicas on one Postgres, a
fake KMS and a fake Google token verifier only at the edges, an agent joins on one replica and a command sent through the other
replica reaches it; killing one replica's listeners fails its readiness and not the other's.

### T4: Identity, sessions and CSRF (S1, S2, S3, S4)

- Decision 1 and 2 as approved: the cookie-session `SessionAuth`, the identity routes (sign-in, callback, sign-out, `me`, access
  requests, user administration), `OidcLogin` from the Entra settings, `Sessions` with its sweeper, `RevocationListener`, `PgUsers`,
  `UserAdmin`, `HubTokenVerifier`, and identity's three audit jobs (checkpoint, nightly verification, log shipping).
- **One CSRF layer over every state-changing route of every module** (D109 closes here). Public routes are listed with a reason.
- The route-table and default-deny checks that `hub-identity` exports (`assert_hub_route_table`, `assert_default_deny`) run in `hub`'s
  tests over the composed router.

**Verify:** the checks above pass on the composed router; a test fails if any non-GET route is reachable without the CSRF token or
the `Origin` check, over every route of every module (identity, registry, writes, platform, MCP); signing in against a local
test identity provider yields a session cookie that works on a registry route, and signing out ends it and its live stream.

### T5: Registry and write paths (S4, S8, S9, S10)

- A registry module (`registry_routes`) and a writes module (`routes::mount` with `WriteState`), both behind the same guard.
- The ingestion sinks (`AgentIngest`, `SentinelIngest`) handed to the agent and sentinel services; `Pipeline` and the registry's
  eleven jobs registered with their lease specs; `RegistryService` and its warm-up; the settings indexer; `RegistryBlobPolicy` on the
  blob endpoint; `PgToolWrites`.
- `WriteHub` with the vault (`CloudKmsEnvelope`, `GithubClient`), the PR jobs, `TeamsNotifier`, the proposal expirer, the credential
  cleanup, the PR-state watcher and the auto-notifier; the four job wrappers of decision 7; `PrLinks` on the webhook.
- Configuration structs (`RegistryConfig`, `WriteConfig`, `PrSettings`, `TeamsConfig`, `GithubClientConfig`) built from the settings
  with the documented defaults.

**Verify:** integration tests on real Postgres: a report from an agent that joined through the real listener lands in the registry
and a read route returns it; a write path runs end to end with GitHub faked at the HTTP edge, writes exactly one audit event per
state change in its transaction, and its job runs under a lease on one replica only.

### T6: MCP and documents (S14, S14b, S15)

- `McpState::new` with the credentials of decision 12, the `OAuthSettings` and the public origin from the settings,
  `PersonalTokens::load`, `PgTokenStore`; `McpModule` behind the CSRF layer where D109 requires it.
- The `Unavailable` `DocSearch` of decision 5. The docs tools return a typed "not available" and the REST docs operations stay in the
  allowance list.

**Verify:** the existing `hub-mcp` auth and route-table tests pass against the composed hub; a call to a docs tool returns the typed
refusal and never data; `mcp_module.rs` asserts the CSRF layer.

### T7: Registering swimlanes (S10; rule 8)

- `hub seed <file>`: a declarative, idempotent command that registers projects, swimlanes (with `agent_service_account`), sentinels
  (with `service_account`) and swimlane tenants from a YAML or JSON file, validated against the existing column constraints, with a
  `--dry-run` that prints what would change. It runs as `lanekeeper_app`, so it can only do what the app role may.
- **Exactly one audit event per changed row, in the same transaction** as the change (an action for it, in `AuditAction`, may need a
  contract change: list it in the plan as `Contract-change:` and stop for approval). Re-running with the same file changes nothing and
  writes nothing.
- A synthetic example file only (rule 12); real swimlane files live outside the repository.

**Verify:** against real Postgres: a new file creates the rows and the audit events and the chain verifies; a second run is a no-op;
a bad service-account email, an unknown project and a duplicate id are refused with typed errors and change nothing; an agent whose
service account was seeded can `Join` through the T3 listener.

### T8: Whole-process proof and the gate

- The full route-table check over `api/openapi.yaml`: every one of the 99 operations is either mounted with a guard or named in an
  explicit spec-only allowance list with its reason and owner (decisions 6 and 13); a new unlisted route or a new unlisted operation
  fails the test.
- A startup benchmark (extend `benches/startup.rs`, registered in `perf/budgets.toml` only if the plan lists the contract change)
  or a measured test showing P11's 10 seconds with the real composition against a database of the fixture size; the memory figure
  recorded.
- `just dev` starts Postgres, runs migrations and the seed with the synthetic fixtures, starts the hub and serves the web app.
- `just verify-18` (formatting, clippy, the crate's tests, the route-table and CSRF tests, the settings and hardening tests, the
  migration and seed tests, `cargo deny`) and a short list for prompt 09: the environment variables, secrets, mounts, the migration Job,
  the seed Job and the `/readyz` contract that the chart must provide.
- The hub-assembly section of `docs/threat-model.md`: the guard and CSRF design as built, the cookie adapter, the unavailable docs
  search, the listeners' trust, what the composition does not prove.

**Verify:** `just verify-18` passes; the kind test of prompt 09 reaches the hub stage (record what it shows, even if the chart needs
changes that belong to 09).

## Acceptance (all required)

- [ ] `just verify-18` passes, and `just verify` passes.
- [ ] The binary starts against a real Postgres, serves `/readyz` 200, and exits cleanly on SIGTERM within the grace period.
- [ ] It refuses to start, with a typed message and no secret, when the database, the CA, the identity provider settings or a required
      module is missing; no fake is reachable from a release build.
- [ ] One test proves every operation of `api/openapi.yaml` is mounted with a guard or named in the allowance list, and every
      state-changing route is behind CSRF and `Origin` checks.
- [ ] An agent whose swimlane was seeded joins through the real agent listener and its report reaches a registry read.
- [ ] `hub migrate` and `hub seed` are idempotent, audited where they change state, and refuse the wrong role.
- [ ] Startup is under 10 seconds with the real composition, and memory is recorded against P11.
- [ ] `docs/threat-model.md` has the assembly section; the decisions made at the plan gate are in `docs/decisions.md`.

## Stop and escalate if

- Decision 1 cannot be met without changing `hub-identity` or `hub-platform` (a contract-like change in another prompt's crate): write
  the exact change and wait.
- A real component cannot be built from settings alone (a missing port implementation, a key that has no source).
- A security requirement conflicts with a functional one, for example a route that must be public.
- The startup budget cannot be met (include measurements).
- You need a new audit action, port method or migration (`lanekeeper-contract-change`).

## Out of scope

Prompt 14 (document search itself); the unbuilt admin and docs operations (D112); the Helm chart, the Jobs for `hub migrate` and
`hub seed`, and the cluster's side of the settings (prompt 09, using the list T8 hands over); the web app; changes to the other
crates, except those a human approves in the plan.
