# Hub assembly: what exists and what is missing (survey of 2026-10-26)

A reference for `prompts/18-hub-assembly.md`. An exploration agent read the repository at `origin/main` (`6ac6019`) on 2026-10-26 and
reported the state below. It ran no code. **READ** means the agent saw it in code or a document; **INFERENCE** means it is the agent's
reading. Counts marked "derived" came from a script or grep. Treat this as a starting point: the plan for prompt 18 re-verifies
what it relies on, and a statement here that the code contradicts loses.

## Headline findings

1. `crates/hub/src/main.rs` is a stub (READ). It reads `HubSettings`, starts telemetry, logs "no identity module (prompt 03a) and no
   registry (prompt 05) linked in" and exits 1. It is a plain `fn main`: no Tokio runtime, no listeners, no signal handling.
2. `Module`, `Deps` and `Hub::build` are done and tested (READ). Four modules exist: `EventsModule`, `GitModule` (with
   `MirrorsReady`), `BlobsModule` and `McpModule`. There is no identity, registry, writes or docs module.
3. **Two incompatible `GuardedRouter`s** (READ). `hub::Module::mount` takes `hub_platform::GuardedRouter`
   (`crates/hub/src/compose.rs`, `crates/hub-platform/src/routes.rs`): no CSRF check, it authenticates through
   `Arc<dyn SessionAuth>`. `hub_identity::guard::GuardedRouter` (`crates/hub-identity/src/guard/router.rs`) is a different type:
   it owns a CSRF layer, an `AuthLayer` over `Arc<Sessions>`, a default-deny fallback and its own route table.
   `identity_routes(...)` takes that type. `hub-identity` depends only on `domain` and `ports`, so it cannot implement
   `hub_platform::SessionAuth`.
4. **No cookie-session `SessionAuth` exists** (READ). The only non-fake implementor is `hub_mcp::auth::BearerAuth` (bearer
   tokens). `plans/03b-hub-platform.evidence.md` (lines ~57 and ~61) calls "which of the two GuardedRouters survives" a human
   decision, and notes `SessionAuth::still_valid(&UserId)` is per user while sessions are per cookie, so a plain logout does not
   end that browser's SSE stream.
5. `hub-docsearch` is an empty crate (READ). `ports::DocSearch` has only a fake, and `McpDeps.docs` requires an
   `Arc<dyn DocSearch>`. Prompt 14 has no plan or evidence yet.
6. **Nothing runs migrations outside tests** (READ). The hub's `sqlx` has no `migrate` feature, `xtask` has no migrate command,
   and `hub ca-init` is named in a comment only.
7. **Nobody owns the assembly** (READ). Every owner's PR defers it to "09" or to an unnamed follow-up; prompt 09's own text
   excludes application code (see "Who was meant to assemble it" below).
8. By the agent's count (derived), 50 of the 99 OpenAPI operations have no route in any crate: 33 under `/api/v1/admin/*`,
   10 under `/api/v1/docs*`, and 7 others. D112 says the admin operations are "in the specification and nobody owns them".

## `crates/hub` today (READ)

- **Dependencies:** `domain`, `ports`, `hub-platform`, `hub-mcp`. Not `hub-identity`, `hub-registry`, `hub-writes`,
  `hub-docsearch`, `engine` or `proto`. The root `Cargo.toml` `[workspace.dependencies]` lacks `hub-registry`, `hub-writes` and
  `hub-docsearch`. `crates/hub/tests/release_build.rs` fails if `crates/hub` or `crates/hub-platform` `[dependencies]` enable
  `fakes` or `conformance`. `sqlx` features: `postgres`, `runtime-tokio`, `tls-rustls-ring-native-roots`.
- **No dependency cycle:** no crate depends on `hub`; `hub-identity` depends on `domain` and `ports` only; `hub-registry`,
  `hub-writes` and `hub-mcp` depend on `hub-platform`. Caveats: `hub-identity`'s `route-table` feature pulls `ports/conformance`,
  so it must stay dev-only; the dev features of `hub-registry` and `hub-writes` (`fakes`, `test-doubles`, `bench-probe`,
  `fuzz-hooks`) must stay off.
- `compose.rs`: `trait Module { name(); mount(GuardedRouter) -> GuardedRouter }`;
  `Deps { settings, auth: Arc<dyn SessionAuth>, modules, static_files, checks }`; `Hub::build(deps) -> Result<Hub, BuildError>`
  gives each module a fresh `GuardedRouter::new(auth, Some(limits))`, merges them, and applies, innermost first: request
  limits, drain gate, error normalisation, security headers, route recording, request context. It also builds the ops router,
  `Drain`, `Metrics`, `Readiness` and `Limits`.
- `serve.rs`: `serve(listener, tls: Option<TlsAcceptor>, router, stop, ServeLimits)` is an axum/hyper accept loop with a
  connection semaphore, handshake and header timeouts and graceful drain. `tls13_server_config(chain, key)` is TLS 1.3 only with no
  client auth. **No code reads a certificate or key from anywhere.**
- `config.rs`: `HubSettings::from_lookup` reads `LK_WEB_ADDR`, `LK_OPS_ADDR`, `LK_MIRROR_DIR`, `LK_WEB_DIST`,
  `LK_TRUSTED_PROXY_HOPS`, `LK_TRUSTED_TYPES`, `LK_MAX_CONNECTIONS`, `LK_BODY_MAX_BYTES` (default 4 MiB),
  `LK_REQUEST_TIMEOUT_SECS`, `LK_JSON_DEPTH`, the IP and user rate settings, `LK_BLOB_CACHE_MIB`, `LK_OTLP_ENDPOINT`, the shutdown
  grace and drain, the database group (`LK_DATABASE_URL_FILE` or `LK_DATABASE_URL`, `LK_DB_CA_FILE`, pool and timeout settings) and
  the Git group (`LK_GIT_*`, `LK_TREE_INDEX_ENTRIES`). Nothing is required except `LK_DB_CA_FILE` when a database URL is set.
  **Not read:** the agent (8444) and replica (8445) addresses, any TLS certificate or key, the CA keys and certificates, the replica
  name and advertised address, the hub audience, the GCP project, KMS keys, Entra, the public origin, the checkpoint bucket, the
  GitHub API and Teams settings. `settings.git` is consumed by no code in `crates/hub`. `hub-mcp` reads its own `LK_MCP_AUTH`,
  `LK_MCP_CONFIRM`, `LK_MCP_CONTENT`. No other library crate reads the environment.
  `crates/hub/tests/hardening.rs` requires every `LK_*` named in `HARDENING.md` to be a string literal in `src/config.rs`, and
  rejects env names containing PASSWORD, TOKEN, SECRET or CREDENTIAL or ending `_KEY`.
- `db.rs`: `connect(&DatabaseSettings)` builds a `PgPool` with `verify-full`, the CA file, a statement timeout and bounded lifetimes;
  `PostgresReady` is a `ReadyCheck`. `ops.rs`: `/healthz`, `/readyz`, `/metrics`; `Readiness` has a 2 s overall deadline. Real checks:
  `PostgresReady` and `MirrorsReady`.
- `shutdown.rs`: `Drain` with phases Running, NotReady, Stopping. **No signal handling** exists in the library or `main` (only in
  `examples/web_fixture.rs`). `SseState`, `AgentService`, `PgEventBus::start` and others take a `watch::Receiver<bool>` as "closing",
  not a `Drain`; nothing bridges them (INFERENCE: the assembly has to).
- `examples/web_fixture.rs` is a test tool, not a composition (fake session auth, no modules, no database).
  **`crates/hub/benches/startup.rs` `child()` is the closest thing to a composition**: telemetry, `PgPool`, `Mirror::open`,
  `PgEventBus` + `.start(closing)`, `SseState`, `Announcer`, `FetchQueue`/`FetchWorker`/`MirrorSync`, `WebhookState`, `PgBlobStore` +
  `BlobState`, then `Hub::build` with the Events, Git and Blobs modules and the checks `postgres` and `mirrors`. It uses fakes for
  session auth, secrets and credentials, and has no agent or replica listener, identity, registry, writes, MCP or job runner.
  `crates/hub-platform/tests/support/cluster.rs` is the precedent for the gRPC half: per replica `issue_server_identity` ->
  `RotatingServerCert` -> `ServerTls` -> `ClientTls` -> `ConnectionRegistry` -> `ReplicaClient` -> `HubGateway` -> `ReplicaService`,
  and `CaIssuer::new` -> `JoinService` -> `AgentService`, each on a `serve_grpc`.

### Listeners

| Listener | In code | Wired in `main` | Needs |
|---|---|---|---|
| web 8443 (HTTPS, behind the users' load balancer) | `serve()` + `tls13_server_config` | no | A certificate chain and key; no source and no setting (also plan 09 A1) |
| agent 8444 (Agent + Sentinel gRPC, mTLS) | `hub_platform::listener::serve_grpc` over `ServerTls` | no | The agent CA (KMS key + CA certificate) and the hub's server certificate from `issue_server_identity` |
| replica 8445 (forward RPC, mTLS) | same `serve_grpc` + `ReplicaService` | no | The replica CA (second KMS key and certificate, plan A7), replica certificate, `ClientTls` |
| ops 9090 (plain) | `serve(.., None, hub.ops, ..)` | no | Nothing |

`crates/hub/HARDENING.md`: distroless `cc-debian12`-class nonroot image, three StatefulSet replicas, read-only root, ports as above,
probes on `ops`, secrets as files, only four volumes (`tmp`, `mirror`, `db-credentials`, `db-ca`), egress only to DNS, Postgres,
replicas, the telemetry collector, the metadata server and a short FQDN list. **Gap:** `storage.googleapis.com` (needed by
`GcsCheckpointBucket`) is in neither `HARDENING.md` nor the test's host list on `main` (prompt 09's branch adds it). The CA
certificates "arrive through the settings of T3 and T4", but no volume or setting exists.

## Other crates (READ)

| Crate | Exports for assembly | Missing |
|---|---|---|
| hub-identity | `PgAuditLog`, `PgUsers` + `UserAdmin` + `PgGithubTokenPresence`, `Sessions::new` (spawns a sweeper and a last-seen batcher), `RevocationListener::spawn`, `OidcLogin::new(EntraConfig, https, secrets, clock)`, `routes::auth::AuthState`, `IdentityState`, `identity_routes(hub_identity GuardedRouter, ..)` (10 routes), `AuthLayer`, `CsrfLayer`, `HubTokenVerifier` (Entra access tokens + Google ID tokens), `HyperHttpsClient`, audit jobs that spawn themselves (`CheckpointJob`, `NightlyVerifyJob`, `AuditLogShipper`) with `GcsCheckpointBucket` and `MetadataTokenSource`, `IdentityConfig`/`EntraConfig` (serde JSON "loaded by the hub binary") | A `hub_platform::SessionAuth` adapter; a way to mount identity routes into `Hub::build`; CSRF for the rest of the hub; the config loader; `hub` as a dependent |
| hub-platform | GCP clients (`GoogleHttp`, `WorkloadIdentityTokens`, `SecretManagerSource`, `CloudKmsSigner`, `CloudKmsEnvelope`), `CaIssuer`, `issue_server_identity`, `ServerTls`/`ClientTls`, `JoinService`, `AgentService`, `SentinelService`, `ConnectionRegistry`, `HubGateway`, `ReplicaClient`/`ReplicaService`, `PgLeases`, `JobRunner`, `PgEventBus`, `SseState`, `PgBlobStore`, the git mirror/fetch/webhook set, `PrLinks`, `GitCredentials` | Settings for all of it; listener bring-up; the `RegistryBlobPolicy` and `PrLinks` hookups; four platform jobs to register (`outbox.prune`, `webhook-deliveries-prune`, `git-poll`, `git-tree-index`) |
| hub-registry | `AgentIngest`, `SentinelIngest`, `Pipeline` (+4 jobs), `RegistryService` (+`spawn_warmup`), `SettingsIndexer`/`SettingsSweeper`, `GitIndex`, `SyncService`, `Alerts`, `Structure`, `Pickups`, `ServedVerifier`, `Retention`, `PgToolWrites` + matcher, `AdoptionService`, `RegistryBlobPolicy`, `registry_routes` (15 GET operations); eleven leased jobs, each with `spec(&RegistryConfig)` and `job()`; `plans/05-registry-read-api.evidence.md` ("Follow-ups the hub must wire") lists what to wire | A `Module`; the wiring; `RegistryConfig` from settings; a caller for `SyncService::record_sync`; routes for adoption and "mark intentional" |
| hub-writes | `WriteHub::new(WriteHubDeps{..})`, `.token_vault(..)`, `.pr_jobs(..)`, reconciler, PR-state watcher, auto-notifier, proposal expirer, credential cleanup, `GithubClient`, `TeamsNotifier`, `routes::mount` (18 routes) | A `Module`; **`Job`/`JobSpec` wrappers for the workers** (they are bare `run_once` methods; lease names, intervals and ttls are not chosen); wiring of the notifier, watcher and cleanup; config from settings; 8 unrouted operations. `plans/06-write-paths.pr.md` lists the hub work |
| hub-docsearch | nothing | everything (prompt 14) |
| hub-mcp | `McpState::new(McpDeps, McpSettings)`, `Credentials`, `PersonalTokens::load` (secret `mcp-token-hmac-key`), `PgTokenStore`, `McpModule` | Construction in `main`; `OAuthSettings` and the public origin have no env source; a `DocSearch`; the notify-services switch |

## Ports (`crates/ports`)

Real implementations exist for `AuditLog`, `TokenVerifier`, `Users` (hub-identity); `BlobStore` (in Postgres), `EventBus`,
`AgentGateway`, `GitReader`, `KmsSigner`, `KmsEnvelope`, `SecretSource`, `LeaseBackend`/`Leases` (hub-platform); `Notifier`
(`TeamsNotifier`, hub-writes; no no-op notifier exists); `RegistryRead`, `ReportSink`, `SentinelSink`, `ToolWrites` (hub-registry);
`WriteService` (hub-writes). **None: `DocSearch`** (fake only). `TxFactory`/`OwnedTx` have no real implementation and no production
consumer. Non-port traits: `hub_platform::SessionAuth` has **no cookie implementation**; `BlobPolicy` has `RegistryBlobPolicy`;
`ReadyCheck` has two real checks. Every crate defines its own `Clock` trait with a `SystemClock`.

## Database

- Migrations `0001` to `0008` in `db/migrations`. **Nobody runs them in production.** `lanekeeper_migrator` owns the objects and runs
  DDL (`0001_init.sql` raises unless the current user is `lanekeeper_migrator`); `lanekeeper_app` has per-table DML grants only.
  Prompt 09's plan A8 says there is no subcommand and leaves it to "the owner". `db/` is not excluded by the hub's
  `Dockerfile.dockerignore`, so `sqlx::migrate!` embedding is possible (INFERENCE: it is the only way to migrate from a distroless
  image that carries the binary and `web/dist`).
- `swimlanes` (`0001_init.sql`): `id` (`^[a-z0-9][a-z0-9-]{0,62}$`), `project_id` (FK), `display_name`, `cluster`, `tier`,
  `nfs_server`, `nfs_export`, `mount_root`, `agent_service_account`, `editing_unlocked`, baseline columns, `auto_notify`,
  `config_server_started_at`, `created_at`, and `last_notified_at` (`0003`). **No non-test code inserts into `swimlanes`**, `projects`,
  `sentinels`, `join_tokens`, `os_login_user_map`, `path_mappings`, `host_tiers`, `severity_rules`, `merge_modes`, `folder_mappings`,
  `retention_settings` or `notification_settings`; only `swimlane_tenants` is inserted (`hub-registry/src/adoption/confirm.rs`). Those
  rows exist only through SQL, and the admin APIs that would write them are the unbuilt 33 operations.
- `hub-platform/src/join/google.rs` `registered_service_account(pool, swimlane)` selects `swimlanes.agent_service_account` and compares
  the verified Google email in constant time. A swimlane with none cannot join this way, so **until swimlane and sentinel rows are
  seeded, `Join` always fails** (INFERENCE).

## Audit in the same transaction

`ports::AuditLog::record(&self, tx: &mut Tx<'_>, e: AuditEvent)` (and `record_many`, at most 200 events and 1 MiB). The caller opens a
transaction, makes its state change, calls `record` **last** (it takes the chain lock, which is held until commit), then commits. An
example is `crates/hub-mcp/src/auth/pg_store.rs`. Outbox events go through `EventBus::publish_in_tx`. `AuditEvent::new(at, actor, action,
via)` with optional `swimlane`, `path`, hashes, `diff` (at most 64 KiB, flagged values redacted), `github_login`, `approval`, `author`,
`request_id`, `attribution`. `PgAuditLog::record` needs a Postgres `Tx`.

## Secrets and cloud

Secret Manager (`SecretManagerSource`, five-minute cache, Workload Identity tokens from the metadata server) is real. Secret names in
code: `github-read-token`, `github-webhook-secret` (defaults), `write-preview-key`, `mcp-token-hmac-key`, the Entra
`login_cookie_key_secret` and client secret or federated token file (Q25), and the Teams destinations named by
`notification_settings.destination_ref`. KMS signing (CA and audit checkpoints) and envelope encryption (the token vault) are real and
need `KeyRef`s that are not settings yet: the agent CA key, the replica CA key (plan A7), the envelope key, the checkpoint key. CA
certificates are public configuration with no loader and no mount. The only GCS code is `GcsCheckpointBucket`. Entra
(`EntraConfig`, `OidcLogin`, `EntraAccessTokens`, `GoogleIdTokens`) is real code with a JSON config shape and no loader. There are two
outbound HTTPS stacks (`hub-identity/src/http.rs`, `hub-platform/src/gcp/http.rs`) and two metadata token sources, and two layers that
write `x-request-id`; the 03b evidence calls this "needs-human (design)".

## Who was meant to assemble it (READ)

- `prompts/03b-hub-platform.md` lists `crates/hub/**` ("binary composition") among 03b's paths; `prompts/03a` says the route-table
  checks are "exported for the hub binary"; prompt 09 owns `deploy/**` and `.github/**` and says "Out of scope: application code".
- `plans/03b-hub-platform.md` (A5) assumed each owner adds a line to `compose.rs`. 03a did not; 07 did (`McpModule`); 05's A8 says
  "Not 05"; 06's A15 leaves it "to the owner of `crates/hub`". `plans/03b-hub-platform.pr.md` says "Prompt 09 composes the binary" and
  `plans/03a-hub-identity-audit.evidence.md` says it "depends on prompt 09". D109 (`docs/decisions.md`) says the MCP CSRF deviation is
  closed by "the hub integration, the PR that composes `McpModule`", which must wrap every non-GET route in 03a's `CsrfLayer`. D112 says
  the registry wiring is "one follow-up once 03a, 05 and 06 have merged". `docs/threat-model.md` says "Until 09 wires the checks, the
  guarantee covers this crate's routes alone" and that the binary "refuses to start" without the real identity module and registry.
- Prompt 09's plan has it as **A1** ("Who composes the hub binary? ... A human assigns composition (a new prompt, or Extra-paths
  `crates/hub/src/**` plus a task)") and **A8** (migrations). Its acceptance 3 (a kind cluster with `/readyz` on all three replicas) is
  NOT MET because of it.

## Existing tests

`crates/hub/tests`: `guard.rs` (the platform guard), `mcp_module.rs` (the only test that runs `assert_route_table` against
`api/openapi.yaml`, with spec-only allowances for everything else; it does not assert the CSRF layer D109 requires), `blob_route.rs`,
`events_route.rs`, `git_route.rs`, `ops.rs`, `mirrors_ready.rs`, `hardening.rs`, `release_build.rs`, `headers.rs`, `limits.rs`,
`errors.rs`, `static_files.rs`, `telemetry.rs`, `tls.rs`, `pg_tls.rs`, `justfile_gate.rs`. Each crate has its own route-table test that
allows the other crates' operations, so **nothing checks the full 99-operation table**, and **no test composes the real process**. The
P11 startup bench is the only multi-module composition.

## The startup sequence the code implies (INFERENCE)

1. A multi-thread Tokio runtime; parse the settings and exit 2 on error; telemetry; install signal handlers before the first poll (the
   03b blockers file records a SIGTERM race).
2. Migrations, as `lanekeeper_migrator`, before the `lanekeeper_app` pool serves.
3. Google plumbing: Secret Manager, KMS signer, KMS envelope.
4. `db::connect`; `PgAuditLog`, `PgLeases` (with the replica's `PeerName`), `PgEventBus` + `.start(closing)`.
5. Git: `Mirror::open`, `spawn_initialize`, `GixReader`, `Announcer`, `FetchQueue`/`FetchWorker`, `MirrorSync`; `PgBlobStore`.
6. Two `CaIssuer::new` (they probe KMS and refuse to start on a mismatch), `issue_server_identity` x2, rotation tasks, `ServerTls`,
   `ClientTls`, `ConnectionRegistry`, `HubGateway`.
7. Identity: HTTPS client, `OidcLogin::new`, `Sessions::new`, `RevocationListener`, `PgUsers`/`UserAdmin`, `HubTokenVerifier`.
8. Registry services, then writes (hub, vault, PR jobs), `TeamsNotifier`, `PersonalTokens::load`, `McpState::new`.
9. `JobRunner`: register the platform (4), registry (11) and writes (4) jobs; start identity's three self-spawned audit jobs; the plain
   tasks (PR-state watcher, auto-notifier, registry warm-up).
10. `JoinService`, `AgentService.with_gateway`, `SentinelService`, `ReplicaService`.
11. `Hub::build(Deps { auth, modules, checks, .. })`.
12. Bind the four listeners with `stop = drain.stopping()`; readiness goes 200 after the mirrors are initialised.
13. On SIGTERM: `drain.shutdown(grace)`, flip `closing`, stop the job runner, wait for the listeners to drain (25 s default), flush
    telemetry, exit 0.

## Open questions the work order has to settle

See "Decisions at the plan gate" in `prompts/18-hub-assembly.md`; the list there is this survey's list with proposed defaults.

## Size (the agent's ESTIMATE, not measured)

About 70 constructor calls; about 19 leased jobs, 3 self-spawned identity jobs and 12 plain tasks; 4 listeners; 3 or 4 new `Module`s
plus an auth adapter; 30 to 40 new settings; 8 to 12 new source files in `crates/hub/src`, 5 to 8 new test files; 3,000 to 6,000 lines
of code and tests. Unbuilt prompts or owners (14, the admin APIs) add work outside this assembly.

## Corrections after verification (prompt 18, T8)

The survey was written by an agent that ran no code, and said so. Prompt 18 re-read what it relied on (the plan's table "What I
re-verified" has each line with its evidence) and then built the assembly. This section is what turned out different, in the order the
survey was wrong or short, so that the next reader does not repeat a claim that was disproved. Line numbers are gone because the
code moved; the decisions and tests named are where to look.

**Wrong, or incomplete in a way that changed the design.**

- **"One `GuardedRouter` per crate, pick one."** There were two incompatible types, and the platform's was typed into four crates
  (registry, writes, MCP, and the platform's own routes), so it was the only one that could survive. Identity's router also carried a
  custom fallback that axum would adopt silently or panic on when merged: it is carried as a mounted sub-router with its fallback taken
  off (`reset_fallback`). `CsrfLayer` does nothing unless identity's `AuthLayer` runs outside it, so a hub-wide CSRF layer needed a
  hub-wide `AuthLayer` (D141).
- **"`SessionAuth` for cookies is missing."** True, and the platform's trait wants a whole `User` while identity's session cache holds
  ids, a role and a status. The adapter (`CookieAuth`) decides on the role and status in the request's `AuthContext` and takes the
  profile from a bounded user cache evicted by the same event, because an uncached user read per request would be most of P7's 2 ms
  (CC1: `P7.auth_composed`, `P7.auth_composed_cold`).
- **"End the stream on the revocation event for that session."** The event carries a user id, never a session id. The stream is bound
  to its session by the SHA-256 of the session id and asks on its heartbeat (D143, three `hub-platform` files).
- **"The migrations can be checked by the migration table."** The application role cannot read `_sqlx_migrations`. The hub probes the
  catalog for one object per migration (D138).
- **"The CA material is two key refs and two certificates."** Nothing could make a CA certificate (`hub ca-init` was named in a
  comment and not built), and the agent listener's certificate needs the hub's agent-endpoint host names (`LK_AGENT_ENDPOINT_HOSTS`).
- **"Identity's metrics register through `Metrics::register`."** They are plain atomics behind `CounterSource`; a collector in the hub
  exports them. `hub-writes` has no metrics at all.
- **"Run `assert_hub_route_table` and `assert_default_deny` in the hub's tests."** Neither can run on a composed router that serves the
  web app: `assert_default_deny` expects `401` for an unregistered path, and the composed hub answers the shell or `404`. The same
  facts are asserted with the ports harness and a hub-owned default-deny test, with `/auth`, `/mcp` and `/.well-known` reserved beside
  `/api` and `/hooks` so that no unknown path under them is answered with the shell.
- **"The hub-writes workers have intervals in D63 and the doc comments."** D63 is the agent's Merkle tree and no doc comment gives
  one; the intervals and lease times are the plan's numbers, as settings within bounds (`job_wrappers.rs`).
- **"`record_sync` has no caller."** The registry's own leased job `registry-sync-watch` runs the pieces of it for closed windows;
  the explicit-commit entry point is prompt 10's. Nothing is left to wire.
- **"`lanekeeper_app` can seed."** Confirmed, and nothing else writes the rows that say which service account may join as which
  swimlane's agent, so `hub seed` was needed for any agent to join (D139).

**Wrong in small ways, or true and worth stating.**

- Fifty of the 99 operations have no route (33 admin, 10 backed by document search, 7 waiting for their owners), which is now a list
  with a reason and an owner (`hub::allowances`, `route_table_full`). Five routes the hub mounts are not operations of the document (`/mcp` on three methods and two OAuth discovery
  documents).
- The web listener's certificate had no source; `RotatingServerCert` already existed in `hub-platform`, and what was missing was a PEM
  decoder at a trust boundary (a fuzz target, `hub_pem`).
- A pod contract that flags any variable whose name contains `TOKEN` or `SECRET` caught two of the Git settings; they are
  `LK_GIT_READ_REF` and `LK_GIT_WEBHOOK_REF` now.
- The binary links both ring (through sqlx) and aws-lc-rs, and only the agent installed a process-default provider; `main` does it
  first, and creates the signal streams before anything is spawned.
- Identity's error bodies are a different shape from the hub's (no `code`, kebab-case in the type, identity's own `req-` request id);
  the API document describes both, and the error test for the composition checks the document's shape, not one layer's.
- `deploy/dev/compose.yaml` starts a Postgres without TLS, which the hub's pool refuses by design (S6), so `just dev` has its own
  development database (`crates/hub/examples/dev/compose.yaml`) with TLS 1.3 and a certificate made by `dev_pki`. Browser sign-in
  cannot be mocked: the discovery check requires the authorize endpoint on the authority's host with no port, which a laptop cannot
  serve; the development example has its own loopback sign-in helper instead (see `crates/hub/README.md`).
- The start-up benchmark of 03b composed a subset by hand (no identity, registry, write paths or MCP) and used `file://` remotes,
  which settings now refuse. The new one runs the example binary through `process::run`, with HTTPS remotes served from the fixtures.

**Size.** The survey guessed 3,000 to 6,000 lines of code and tests; the plan guessed 5,000 to 7,000. The branch changed about 12,600
lines under `crates/hub/src` and about 12,500 under its tests, benches, examples and fuzz crate, because each task was test-first
against a real Postgres and a process, and the documents the tests hold true (the pod contract, the README, the threat model) are
long. No schedule depended on the guess.
