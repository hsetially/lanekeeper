# hub

The Lanekeeper hub: one binary that serves the web app, the API, the live stream, the MCP server, the Git webhook and the blob
endpoint on the web listener, and the agent and replica gRPC services on their own mTLS listeners. The contract for the pod and
the network it runs in is `HARDENING.md`; the design is `plans/18-hub-assembly.md`.

This file says how to operate it and what has to be true around it. `HARDENING.md` is the contract for the pod and the network and has
the detail of every setting; the last section here is the checklist that the deployment (prompt 09's chart and its install guide) is
matched against.

## Commands

```
hub [serve]    run the hub until SIGTERM or SIGINT (the default)
hub migrate    apply the database migrations as lanekeeper_migrator, then exit
hub seed       register the projects, swimlanes, tenants and sentinels that a file lists, as lanekeeper_app, then exit
hub ca-init    print the self-signed certificate of a CA whose key is in Cloud KMS (run once for each CA)
hub help       print the usage
```

Exit codes, for every command: `0` it did what it was asked, `1` it could not (a component it needs was missing or refused it),
`2` the command line or a setting was refused and nothing was started. A message never repeats a value: it names the setting or
the component, and a secret is never in it.

## Operating the hub

### Migrating the database

The hub never changes the schema. It runs as `lanekeeper_app`, which can read and write rows and cannot change a table, so the
migrations are a separate command that runs with a different credential, as a Job, before the hub rolls out. A hub that finds the
schema behind it refuses to start and says which migration is missing:

```
the database schema is not usable: migration 0004 (write additions) has not been applied; run `hub migrate` with the migrator credential first
```

`/readyz` runs the same check (it is named `schema`), so a database that is restored to an older state turns the hub unready
instead of serving errors.

**Run it** from the same image as the hub, with these variables and nothing else of the hub's:

| Variable | Meaning |
|---|---|
| `LK_MIGRATOR_URL_FILE` | The path of a file that holds the connection URL of `lanekeeper_migrator`, password included. A file, never a variable: a password in the environment is visible in `/proc/<pid>/environ` and in a crash dump. Required. |
| `LK_DB_CA_FILE` | The CA that signed the database server's certificate. The same file the hub uses. Required. |
| `LK_MIGRATE_TIMEOUT_SECS` | The longest the whole run may take, connecting, waiting for the lock and applying together. 600 by default, at most 7200. |

```
hub migrate
```

The connection is TLS 1.3 with the server's certificate and name verified against `LK_DB_CA_FILE`, whatever the URL says
(`sslmode=disable` in it is overridden, not obeyed). On a laptop with the dev compose file, `deploy/dev/init.sql` creates the
roles and the passwords are the dev ones in that file.

**What it does.** It takes sqlx's advisory lock on the database, so two Jobs at once (a retry on top of an attempt that is still
running) cannot both apply a migration, applies each missing migration in its own transaction, and prints one JSON line for each
migration it applied and one for the end, in the same format as the serving hub's log (the timestamp is left out here):

```
{"level":"INFO","fields":{"message":"migration applied","version":1,"description":"init"},"target":"hub::migrate"}
{"level":"INFO","fields":{"message":"the schema is up to date","applied":8,"version":8},"target":"hub::migrate"}
```

Run again, it applies nothing and rewrites nothing (`nothing to apply; the schema is complete`, exit 0). The migrations are
compiled into the binary, so the image needs no `db/` directory, and the Job needs no other files than the two above.

**What it refuses**, each with exit 1 and a message that holds a version number or a five-character SQLSTATE and no host, user,
password or word of the database's:

| Refusal | Why |
|---|---|
| a URL whose role is not `lanekeeper_migrator` | Checked before it connects, and again from the session once it has. `lanekeeper_app` and the superuser are refused; the role is a constant in the code, not a setting. Nothing is touched. |
| a database that holds a migration the binary does not know | A newer image already migrated it. Run the Job of that image, not this one. Nothing is changed. |
| a migration that was changed after it was applied | The checksum in `_sqlx_migrations` no longer matches. A merged migration is never edited: add a new file. |
| a migration that fails | It is rolled back whole, and the message gives its version and the SQLSTATE. |
| a run that outlasts `LK_MIGRATE_TIMEOUT_SECS` | The connection is dropped, which rolls back the migration in flight; the session's own `statement_timeout` (the limit plus ten seconds) ends a statement the server is still executing. |

**What it does not do.** It writes no audit event. `audit_events` is created by the first migration, so the first run has no chain
to append to, and the migrator is not the role the audit log is written as. `_sqlx_migrations` records what was applied, when and
with which checksum, and the Job's log has the versions. That a schema change leaves no row in the audit chain is a residual risk
(`docs/threat-model.md`).

**Adding a migration** is a contract change (`db/migrations/` is a contract path; see the `lanekeeper-contract-change` skill). Two
things in this crate must then change in the same commit, and `cargo test -p hub --test migrate` fails until they do:

1. a line in `EMBEDDED` in `src/migrate.rs` for the new file (the test compares the table with `db/migrations/` by name and by
   checksum, so a forgotten, edited or renamed file is caught);
2. a marker in `MARKERS` in `src/schema.rs`: one object the new migration creates and no earlier one does, so that the hub can tell
   whether it has been applied without reading `_sqlx_migrations`, which the app role cannot see (the test applies the migrations
   one at a time and checks each marker belongs to its own).

**Rolling out.** Run the Job of the new image first, wait for it to complete, then roll the hub. Migrations are additive (expand,
then contract), so the old hub keeps working on the new schema while it rolls; the new hub refuses an old schema. In the chart
(prompt 09) this is a pre-install and pre-upgrade hook, or an Argo CD `PreSync` hook, with the Job's pod held to the contract in
`HARDENING.md` ("The migration Job").

### Making the CA certificates

The hub starts only when it has two CA certificates, one for agents and sentinels and one for the replicas, and nothing else makes
them. Each CA is a Cloud KMS key version (algorithm `EC_SIGN_P256_SHA256`, purpose asymmetric signing, a key used for nothing
else); the hub never holds the private key, it asks KMS to sign. `hub ca-init` makes the certificate for a key, once:

```
hub ca-init --kms-ref projects/P/locations/L/keyRings/R/cryptoKeys/agent-ca/cryptoKeyVersions/1 \
            --common-name "Lanekeeper agent CA" --valid-days 3650 > agent-ca.pem
hub ca-init --kms-ref projects/P/locations/L/keyRings/R/cryptoKeys/replica-ca/cryptoKeyVersions/1 \
            --common-name "Lanekeeper replica CA" --valid-days 3650 > replica-ca.pem
```

Run it from the image, as an identity that may read the key's public half and sign with it (`roles/cloudkms.publicKeyViewer`,
`roles/cloudkms.signerVerifier` on that key version). Nothing but KMS is needed, no project and no secret. It prints one PEM
certificate on standard output and its own messages on standard error; store the output in the Secret Manager secret that
`LK_AGENT_CA_CERT_REF` (or `LK_REPLICA_CA_CERT_REF`) names, and then the hub can start. The certificate is public.

| Option | Meaning |
|---|---|
| `--kms-ref` | The key *version*, as a resource name. A key without a version is refused: a signature is made by one version, and the certificate belongs to it. |
| `--common-name` | The certificate's subject, for people: letters, digits, spaces and `. _ -`, at most 64 characters. |
| `--valid-days` | How long the certificate lives from now, 1 to 3650. Choose the date to renew the CA by; renewing is a new key version, a new certificate and a rolling restart. |

The certificate may sign leaf certificates and nothing else (a CA with a path length of 0, key usage `keyCertSign` and `cRLSign`).
Exit codes are the hub's: `0` the certificate is on standard output, `1` KMS refused or was unreachable (the message says which
step), `2` an option is missing or not valid (the message names the option and never the value). The same command line is covered
by the `hub_cli_args` fuzz target. At every start the hub proves with one signature that each certificate belongs to its KMS key and
refuses to start when it does not, so a certificate made for the wrong key, or a secret that holds the wrong one, is found at the
first rollout and not at the first agent.

### Registering swimlanes (`hub seed`)

Until a swimlane has a row with an `agent_service_account`, **its agent cannot join**: `Join` compares the email of the agent's Google
ID token with that column, and a swimlane with none refuses every token, the right one too. The same holds for a sentinel and
`sentinels.service_account`. No route and no job creates a project, a swimlane or a sentinel or writes either account (the admin
operations that would are not built; only the adoption of a baseline adds tenants), so `hub seed` does: it reads a file that lists the
projects, swimlanes, tenants and sentinels and makes the database say what the file says.

**Run it** from the same image as the hub, as a Job or by hand, **as `lanekeeper_app`** with the credential the hub itself uses and
nothing else of the hub's. It checks the role in the URL before it connects and the role of the session after, and refuses
`lanekeeper_migrator`, the superuser and everyone else (exit 1, nothing touched): a seed writes the rows the application role may write
and no more.

| Variable | Meaning |
|---|---|
| `LK_DATABASE_URL_FILE` | The path of a file that holds the connection URL of `lanekeeper_app`, password included: the hub's own Secret mount. (`LK_DATABASE_URL` is for a laptop and is refused beside the file one.) Required. |
| `LK_DB_CA_FILE` | The CA that signed the database server's certificate. The same file the hub uses. Required. |
| `LK_SEED_TIMEOUT_SECS` | The longest the whole run may take, connecting, waiting for locks, reading and writing together. 120 by default, at most 3600. The hub pod must not carry it. |

```
hub seed --dry-run /etc/lanekeeper/seed/seed.yaml     # print what would change; write nothing
hub seed /etc/lanekeeper/seed/seed.yaml               # create and update
```

The file is a mounted file (a `ConfigMap` is right: it holds service-account addresses and names, no secret). **The real file lives
outside this repository** (AGENTS.md rule 12); `examples/seed.dev.yaml` is the synthetic one `just dev` and the tests use.

**The file** is YAML or JSON (JSON is YAML's subset), at most 256 KiB, one document, `version: 1`:

```yaml
version: 1
projects:                              # optional section; each entry: id, display_name
  - id: lanekeeper-dev
    display_name: Lanekeeper (development)
swimlanes:                             # optional section; each entry: id, and what follows
  - id: sl-01
    project: lanekeeper-dev
    display_name: Swimlane 1
    cluster: gke-dev-1                 # optional
    tier: dev                          # optional
    nfs_server: nfs-1.corp.example     # optional
    nfs_export: /export/sl-01          # optional, an absolute path
    mount_root: /mnt/config            # optional, an absolute path
    agent_service_account: agent-sl-01@lanekeeper-dev.iam.gserviceaccount.com   # the file must carry it, or the agent cannot join
    tenants: [sit1]                    # optional: tenants are added, never removed
sentinels:                             # optional section; each entry: vm, and what follows
  - vm: nfs-vm-01
    swimlane: sl-01                    # optional
    export_root: /export               # an absolute path
    service_account: sentinel-01@lanekeeper-dev.iam.gserviceaccount.com   # the file must carry it, or the sentinel cannot join
```

| Where | Key | Needed | Checked as |
|---|---|---|---|
| projects | `id` | always | letters, digits and `. _ -`, starting with a letter or digit, at most 64 |
| projects | `display_name` | to create | text, at most 256 bytes, no control or direction-changing character, no space at either end |
| swimlanes | `id` | always | `[a-z0-9][a-z0-9-]{0,62}` (the column's own check) |
| swimlanes | `project` | to create | a project in the file or in the database |
| swimlanes | `display_name` | to create | text, as above |
| swimlanes | `cluster`, `tier` (at most 64) | no | text, as above |
| swimlanes | `nfs_server` | no | a host name or address |
| swimlanes | `nfs_export`, `mount_root` | no | a clean absolute path: no `..`, `.`, `//`, trailing slash or backslash |
| swimlanes | `agent_service_account` | **in practice, yes** | lower-case, `name@project.iam.gserviceaccount.com` (any `*.gserviceaccount.com`); a person's address is refused |
| swimlanes | `tenants` | no | tenant ids (`[a-z0-9_-]`, at most 63), at most 64, none twice |
| sentinels | `vm` | always | a name the CA puts in a certificate: letters, digits and `. _ -`, at most 253 |
| sentinels | `export_root` | to create | a clean absolute path |
| sentinels | `swimlane` | no | a swimlane in the file or in the database |
| sentinels | `service_account` | **in practice, yes** | as for swimlanes |

At most 200 entries in a section. A key the file does not know (`editing_unlocked: true`, a misspelt `agent_service_acount`) is an
error, never silently ignored. A value that YAML could read as a number (`010`, `1e3`) must be quoted, so that the operator means what
the hub reads. Anchors, aliases, merge keys, a second document, a duplicate key and deep nesting are refused; a tag does not change a
value (`!!binary` is not decoded). A comment is read and thrown away.

**What it does to a running environment: nothing it was not asked to.** The swimlanes of a real environment already exist and run,
so these hold, and `tests/seed.rs` proves each against a real Postgres:

- **A carried field that differs is updated; a carried field that matches is not.** The file is the desired state for the fields it
  carries (including a swimlane's `project`, which moves the swimlane), so read the `--dry-run` before the first real run against an
  environment that is already running. A row that already holds what the file says is not written (not even to the same values) and
  writes no audit event. A service account that differs only in case is the same account. A second run of the same file changes
  nothing and writes nothing, and `--dry-run` says so.
- **A field the file does not carry is never touched**, in a row that exists or in one the seed creates. A swimlane that is already
  running needs only `id` and what you want to set: `cluster`, `nfs_server`, `nfs_export` and `mount_root` are filled by the agent's
  first hello when they are empty, so leave them out and they stay as the agent left them; the baseline, the editing lock, the
  auto-notify switch and every time stamp are not the seed's to write at all. A field cannot be cleared, only set (`null` is "not
  carried"), so a typo is corrected by a new value and never loses data.
- **Nothing is deleted, ever.** What the database has and the file does not list (a project, a swimlane, a sentinel, a tenant of a
  swimlane whose tenants the file lists) is reported on the lines `not in the file, left alone` and is otherwise not touched.
- **`--dry-run` changes nothing and prints exactly what a run would change.** It reads inside a read-only transaction, so the server
  itself refuses a write; the list it prints is the list a run then applies (`tests/seed.rs` holds the two equal).
- **All or nothing.** The whole file is validated, then compared with the database, before the first write; then every write and its
  audit event are one transaction. A run that is refused, or whose audit events cannot be written, leaves no row behind. A run changes
  at most 200 rows, so that the audit chain's lock is held briefly; a bigger file is split.
- **Two at once take turns** (a retry on top of an attempt still running): the second finds what the first committed and has
  nothing to do.

**What it audits.** One audit event for each row it changes, in the same transaction, in one batch: `SentinelEnrolled` for a
sentinel, `SettingsChanged` for a project, a swimlane or the tenants added to a swimlane (one event for that set), by the actor
`System` with the component `hub-seed`, via `system`, filed under the swimlane where the change has one. The event's `diff` is
canonical JSON naming the `kind` (`project`, `swimlane`, `swimlane_tenants`, `sentinel`), the `op` (`create`, `update`, `add`), the
`key`, each column set with what it was and what it became (`{"from": null, "to": "..."}`) and the SHA-256 of the file that asked
for it. The operator who ran the Job is not recorded in the event (the actor is the component); the file's hash is, so keep the files
you apply. Reading `from` and `to` is how an audit answers "who could join as swimlane X, and since when".

**What it prints.** A report on standard output, and one structured line after it. Both name ids and column names, never a value of the
file and never a credential:

```
dry run: nothing was written
  update project dev (display_name)
  add tenants sit5 to swimlane sl-01
  create swimlane sl-02 (project, display_name)
  update sentinel nfs-vm-01 (service_account)
unchanged: 0 projects, 1 swimlane, 0 tenant sets, 0 sentinels
not in the file, left alone: swimlane sl-99
warning: swimlane sl-02 has no agent_service_account, so its agent cannot join
4 rows would change
```

A warning is not an error: a swimlane may be registered now and given its account later, by running the file again. Exit codes:
`0` the database matches the file (whether or not this run changed anything; for a dry run, whatever it would change), `1` the run
could not be completed (the database, the audit log, the role, the time limit), `2` the file or a setting was refused (the message
names the place, such as `swimlanes[2].agent_service_account`, and a fixed reason, and never the value).

**What it refuses**, all with nothing written: a file over 256 KiB or that is not UTF-8, YAML or JSON of this format (a line and
a column are given); a value that is not valid for its column or for what the hub does with it (a service account that is not a
Google service account, a tenant with a dot, a relative path, a name with a direction-changing character); an id listed twice in its
section; a swimlane that names a project, or a sentinel that names a swimlane, that is neither in the file nor in the database; a new
row that lacks a column it needs (`project`, `display_name`, `export_root`); a run that would change more than 200 rows.

**Rolling out.** Run `hub migrate` first, then `hub seed`, and any time after when a swimlane or a sentinel is added or its account
changes: it is safe to run again and to retry. Run `--dry-run` first and read it; the real run prints the same list. The first thing an
agent does is `Join`, so an environment is ready for its agents once this has run with their accounts, and not before.

### Starting the hub

`hub` (or `hub serve`) reads its settings from the environment (`HARDENING.md` lists them, with the pod that carries them), binds
all four listeners, loads what it needs and serves. It exits 2 and starts nothing when a setting is missing or malformed, and exits
1 and names the component when a real one cannot be reached or does not match: the database, the schema (run `hub migrate`
first), the web certificate, either CA, a listener's address. It never falls back to a fake. On SIGTERM or SIGINT it fails
`/readyz`, waits `LK_SHUTDOWN_GRACE_SECS`, stops accepting, gives requests and streams `LK_SHUTDOWN_DRAIN_SECS`, releases the leases
of its jobs, and exits 0; a second signal exits at once with 1.

`/readyz` answers 200 when every one of these holds, and names the first that does not on a 503: `postgres`, `schema`,
`certificates` (the web, agent and replica certificates held are unexpired), `events` (the event bus holds its `LISTEN`
connection), `mirrors` (both Git mirrors hold a branch; a replica on an empty volume fills them first and is not ready until it
has), `jobs` (the job runner is running) and `tasks` (no task that should run for the life of the process has ended). Each answers
from memory, so the probe costs no call.

A ready answer can carry a note. A feature this build does not have is said in the body and never gates readiness: until prompt 14
lands the hub answers `ready (not available: docs_search)`, and `lanekeeper_docs_search_available` is `0`. A probe that reads the
status is unaffected; a person who reads the body, or an alert on the gauge, is told. The five docs tools of the MCP server answer a
typed `unavailable` result in the meantime (`HARDENING.md`, "The document search is not there yet"), never an empty list.

### The MCP server

`/mcp` is served on the web listener, in the one process, as soon as the hub is ready. A client (Cursor) needs: the hub's public
origin (`LK_PUBLIC_ORIGIN`) as the server URL with `/mcp` appended; in OAuth mode (the default) nothing else, because the `401` it
gets carries the address of the discovery document and from there Entra's sign-in; in personal-token mode (`LK_MCP_AUTH=personal_token`)
a token that the person makes in the web app, sent as `Authorization: Bearer lkpat_...`. The two modes are never on together. The
variables, the one secret of personal-token mode (`mcp-token-hmac-key`), the audience and scope the Entra app registration must
expose, and the need for `Mcp-Session-Id` affinity across replicas are in `HARDENING.md`, "The MCP server".

## Local development (`just dev`)

`just dev` starts a hub on a laptop with synthetic data and no Google. It needs Docker, `pnpm` and the Rust toolchain, and it does, in
this order: it generates the small fixtures (`cargo xtask gen-fixtures --scale small`, the synthetic repositories of rule 12) and the
certificates (`cargo run -p hub --example dev_pki`, a CA, a database certificate and a web certificate for `localhost`); it starts a
Postgres 16 with pgvector and **TLS 1.3** (`examples/dev/compose.yaml`, on `127.0.0.1:5433`; the hub's pool verifies the server and has
no mode that does not, so the plain-text database of `deploy/dev` cannot serve); it runs `hub migrate` with the migrator's credential
and nothing else of the hub's; it runs `hub seed examples/seed.dev.yaml` as the application role (four synthetic swimlanes, their tenants
`sit1` to `sit4` and one sentinel); it builds the web app; it copies the fixtures' two repositories into the mirror volume, so that the hub
starts ready without a network; and it runs the **example** `hub_dev`.

`hub_dev` is the same `hub::process::entry` as the binary, with software where the binary has Google (`examples/support/dev_edges.rs`):
secrets are files in `target/dev/secrets`, the two CAs' keys and the audit checkpoint key are software keys made at start, GitHub is a
port nobody listens on, the Entra provider is unreachable, and no Google ID token is accepted. It is an example target: `cargo build
--release -p hub` builds no example, the image has none, and `tests/release_build.rs` and `just hub-lint` fail if a test-only feature
reaches a release build. No agent is started and no webhook is exercised.

The web app is at **https://localhost:8443** (trust `target/dev/pki/ca.pem` or accept the warning). A browser cannot sign in to a mock
Entra (the discovery check demands the authorize endpoint on the authority's host with no port), so the example has its own way in:
`http://localhost:8480/` makes a development administrator and a session for it through the hub's own `Sessions` and sends the browser to
the hub with the two cookies (`examples/support/dev_login.rs`; loopback only, and it answers a loopback `Host` and nothing else). Whether
a browser accepts a `Secure` cookie from `http://localhost` is the browser's decision; `curl -i http://localhost:8480/` prints the same two
`Set-Cookie` lines to set by hand. Remove `LK_DEV_LOGIN_ADDR` from `examples/dev/dev.env` to run with no way in at all. `just dev-down`
stops the database; its volume, and the generated files, are under `target/dev` and the Docker volume `lanekeeper-hub-dev_pgdata`.

## What the chart and the install guide must match (for prompt 09)

Everything below is a fact of the code at the commit that wrote it; `HARDENING.md` has the detail and `tests/hardening.rs` holds the
pod and the policy to it. Where this list and a chart differ, the code wins, and the chart or this list is wrong.

**1. One image, four commands.** The image runs `hub` (the default, `serve`) in the StatefulSet. The same image runs `hub migrate`
(a Job), `hub seed <file> [--dry-run]` (a Job) and `hub ca-init --kms-ref ... --common-name ... --valid-days ...` (a person, once for each
CA). Exit codes everywhere: 0 done, 1 could not (a component refused or was unreachable), 2 a setting or the command line was refused.

**2. The order of a first deployment.** (a) The cloud pieces below. (b) The CNPG cluster with the two roles and the extensions
(`deploy/dev/init.sql` is the shape). (c) `hub ca-init` for the agent CA and for the replica CA, from two different keys, each output
stored in its Secret Manager secret. (d) The migration Job, to completion. (e) The StatefulSet (the hub refuses to start on an unmigrated
database and says which migration is missing). (f) The seed Job, with a reviewed file that carries every swimlane's
`agent_service_account` and every sentinel's `service_account`: until it has run, nothing can join, and the hub serves with no swimlanes
meanwhile, so the rollout need not wait for it. A new swimlane later is a new seed run. Helm hooks (`pre-install`, `pre-upgrade`) or Argo
`PreSync` for (d); (f) is a Job, not a hook.

**3. The cloud pieces, made outside the hub.** Cloud KMS: the agent CA key version and the replica CA key version (asymmetric signing,
`EC_SIGN_P256_SHA256`, used for nothing else), the audit checkpoint key version (the same algorithm; older versions may be named for
verification), and the vault crypto key *without* a version (symmetric). A Cloud Storage bucket **with a retention lock** for the
checkpoints (set by whoever owns the project, not by the hub). Secret Manager secrets and their contents, by the names the settings give:
the web certificate chain and its key (`LK_WEB_CERT_REF`, `LK_WEB_KEY_REF`; rotated by writing a new version, picked up within
`LK_WEB_CERT_RELOAD_SECS`), the two CA certificates (`LK_AGENT_CA_CERT_REF`, `LK_REPLICA_CA_CERT_REF`), the Entra client secret
(`LK_ENTRA_CLIENT_AUTH_REF`), the login cookie key (`LK_ENTRA_COOKIE_KEY_REF`, 32 random bytes as base64url), `github-read-token`,
`github-webhook-secret` (the names `LK_GIT_READ_REF` and `LK_GIT_WEBHOOK_REF` override), `write-preview-key` (at least 32 bytes, **read at
start: the hub does not start without it**), `mcp-token-hmac-key` (only in `personal_token` mode), and one secret per Teams destination,
named by an admin in `notification_settings`.

**4. What the pod's Google identity (Workload Identity) may do, and no more.** `roles/cloudkms.signerVerifier` on the two CA key versions
and the checkpoint key version(s); `roles/cloudkms.cryptoKeyEncrypterDecrypter` on the vault key; `roles/secretmanager.secretAccessor` on
the secrets above; on the bucket `roles/storage.objectCreator` and `roles/storage.objectViewer` (not `objectAdmin`, no delete). The
person who runs `hub ca-init` needs `roles/cloudkms.publicKeyViewer` and `roles/cloudkms.signerVerifier` on that one key. The pod mounts
no service-account token (`automountServiceAccountToken: false`); Workload Identity reaches Google through the metadata server.

**5. The pod.** Exactly the Pod block of `HARDENING.md`: three replicas as a StatefulSet (the stable pod name is `LK_REPLICA_NAME`, from the
downward API, the one `valueFrom` the contract allows; `LK_REPLICA_ADDRESS` is `$(LK_REPLICA_NAME).<headless service>.<namespace>.svc:8445`,
lower case); four ports (web 8443 HTTPS, agent 8444 mTLS, replica 8445 mTLS, ops 9090 plain HTTP); `terminationGracePeriodSeconds` 45
(it must exceed `LK_SHUTDOWN_GRACE_SECS` plus `LK_SHUTDOWN_DRAIN_SECS` plus 10; the test holds it); non-root `65532`, read-only root
filesystem, all capabilities dropped, `RuntimeDefault` seccomp; **four volumes only**: `/tmp` (an `emptyDir`, 256 MiB), the mirror volume
(a PVC for each replica, `LK_MIRROR_DIR`), the database credential (a read-only Secret, mode `0440`, `LK_DATABASE_URL_FILE`) and the
database CA (`LK_DB_CA_FILE`). Probes: liveness `/healthz`, readiness `/readyz`, both on the `ops` port. No variable holds a secret
(`HARDENING.md` lists the names to avoid), and `LK_MIGRATOR_URL_FILE`, `LK_MIGRATE_TIMEOUT_SECS` and `LK_SEED_TIMEOUT_SECS` are the Jobs'
alone.

**6. The two Jobs.** `migrate`: the migrator's Secret (`LK_MIGRATOR_URL_FILE`, mode `0440`) and the database CA, nothing else of the hub's;
egress to Postgres and DNS only; no ports. `seed`: the application role's Secret (the one the hub mounts), the database CA, the seed file
from a `ConfigMap` at the path in its arguments; egress to Postgres and DNS only. Neither is given the other's credential, and the code
refuses it. Both: the hub pod's security context unchanged, a retry is safe, two at once take turns.

**7. The settings, by who sets them.** *Required for a start* (exit 2 if absent): the 25 names in `REQUIRED_VARIABLES` of
`tests/hardening.rs`; the Pod block carries those and a few optional ones. *Optional, with the design's defaults*: the MCP group (`LK_MCP_*`), the registry and
write-path switches (`LK_RETENTION_ENABLED` off, `LK_RETENTION_DRY_RUN` on, `LK_EXTRA_DENY_GLOBS`, `LK_TEAMS_ALLOWED_HOSTS`,
`LK_GITHUB_API_BASE`, `LK_GIT_BASE_BRANCH`, `LK_CONFIG_SERVER_DEPLOYMENT`, `LK_WRITES_*_SECS`, `LK_CORPUS_CACHE_MIB`), the identity extras
(`LK_BOOTSTRAP_ADMINS`, `LK_ENTRA_AUTHORITY_HOST`, `LK_ENTRA_REDIRECT_PATH`, `LK_ENTRA_API_AUDIENCES`,
`LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS`), and the tunables in the table below. Change a tunable only with a number in hand.

| Tunable | Default (range) | What it bounds |
|---|---|---|
| `LK_DB_MAX_CONNECTIONS` | 16 (1 to 512) | The pool of each replica. Three replicas times this, plus the Jobs' and an operator's, must fit the database's `max_connections`. |
| `LK_DB_ACQUIRE_TIMEOUT_SECS`, `LK_DB_STATEMENT_TIMEOUT_SECS` | 5 (1 to 60), 30 (1 to 600) | Waiting for a connection, and one statement. |
| `LK_START_TIMEOUT_SECS` | 8 (1 to 120) | Each step of starting (P11's 10 seconds is the whole). |
| `LK_SHUTDOWN_GRACE_SECS`, `LK_SHUTDOWN_DRAIN_SECS` | 5 (0 to 60), 25 (1 to 120) | Readiness fails first, then connections drain. |
| `LK_BODY_MAX_BYTES` | 4 MiB (1 KiB to 64 MiB) | The global body cap (D137 gives two routes more, for a live Editor session). |
| `LK_REQUEST_TIMEOUT_SECS`, `LK_JSON_DEPTH` | 30 (1 to 600), 64 (1 to 256) | A request's whole exchange; the nesting of a JSON body. |
| `LK_IP_RATE_PER_SEC`, `LK_IP_BURST`, `LK_USER_RATE_PER_SEC`, `LK_USER_BURST` | 50 and 200, 30 and 120 | The per-address bucket (outside authentication) and the per-user bucket. |
| `LK_BUCKET_CAPACITY`, `LK_MAX_CONNECTIONS` | 100,000 (100 to 10,000,000), 4,096 (1 to 1,000,000) | The bucket tables, and the connections one replica serves at once. |
| `LK_BLOB_CACHE_MIB` | 512 (1 to 4096) | The blob cache. |
| `LK_TREE_INDEX_ENTRIES`, `LK_GIT_QUEUE_CAPACITY`, `LK_GIT_FETCH_TIMEOUT_SECS` | 400,000, 256, 120 | The tree-index cache, the pushes that can wait for the fetch worker, one fetch. |
| `LK_TRUSTED_TYPES` | `lanekeeper-docs,lanekeeper-monaco` | The Trusted Types policy names in the CSP; the web build must create exactly these. |
| `LK_GIT_TOKEN_FILE` | unset | Q17's interim: a read-only Git credential from a mounted file **instead of** Secret Manager. It needs a fifth volume, which the contract refuses; use `LK_GIT_READ_REF`. |

**8. The network.** The NetworkPolicy and the FQDN policy of `HARDENING.md` exactly: ingress web 8443 from the users' load balancer range, agent
8444 from the agent load balancer's range, replica 8445 from the hub's own pods, ops 9090 from `monitoring`; egress DNS, the database,
the other replicas, the OpenTelemetry collector, the metadata server, and 443 to `login.microsoftonline.com`, `github.com`,
`api.github.com` (or the GitHub Enterprise host in `LK_GITHUB_API_BASE`), `secretmanager.googleapis.com`, `cloudkms.googleapis.com`,
`www.googleapis.com`, `storage.googleapis.com` and the Teams webhook host. The GitHub hook ranges on `/hooks/github` are the chart's (S18).

**9. The users' load balancer** must: pass `Cookie`, `Origin` and `X-CSRF-Token` unchanged and not cache `/api` or `/auth`; pass a
request body of 12 MiB + 64 KiB on `PUT /api/v1/swimlanes/{id}/files` and `POST /api/v1/drafts` and a timeout no shorter than
`LK_REQUEST_TIMEOUT_SECS`; not buffer or cut `text/event-stream` (`/api/v1/events`, `/mcp`) before the hub's 15-second keep-alive;
keep an MCP client on one replica by its **`Mcp-Session-Id`** header (not verified for Google's external HTTPS load balancer; D142);
count two proxies for `LK_TRUSTED_PROXY_HOPS` as the contract says; and serve the name users type, which is `LK_PUBLIC_ORIGIN` (the audience agents ask Google for, `LK_HUB_AUDIENCE`, is the hub's own name and is normally the same).
The agent load balancer passes TCP to 8444 and does not terminate TLS (the agents verify the hub's certificate by the names in
`LK_AGENT_ENDPOINT_HOSTS`).

**10. The Entra app registration.** Single tenant (`LK_ENTRA_TENANT_ID`); redirect URI `LK_PUBLIC_ORIGIN` plus `/auth/callback`; a client
secret stored in Secret Manager (the federated credential is off until Q25 is answered); for MCP, the application ID URI
`api://<client id>` (or the first `api://` entry of `LK_ENTRA_API_AUDIENCES`), the scope `mcp.access`, v2 access tokens
(`accessTokenAcceptedVersion: 2`), and Cursor's client ID pre-authorized for the scope (D91). The first administrators are the Entra object
ids in `LK_BOOTSTRAP_ADMINS`; everyone else starts pending.

**11. The `/readyz` contract.** `200` with the body `ready` when `postgres`, `schema`, `certificates`, `events`, `mirrors`, `jobs` and `tasks`
all hold; `503` naming the first that does not. A ready hub can say `ready (not available: docs_search)`: a note that never gates and that
the chart must not treat as a failure (`lanekeeper_docs_search_available` is `0` until prompt 14). A replica on an empty mirror volume is
not ready until both repositories are fetched, which depends on the network and is outside P11. `/healthz` is `200 ok` while the process
serves. `/metrics` is Prometheus text on the ops port, with route templates as the only labels.

**12. Sizing and alerts worth having.** Requests 500m CPU and 1536 MiB, limits 4 CPU and 2 GiB (the Pod block; P11 budgets 1536 MiB for the
whole hub at design scale, and prompt 16 measures it). The database needs room for `3 x LK_DB_MAX_CONNECTIONS` plus the Jobs' and an
operator's. Alerts: `lanekeeper_docs_search_available == 0` (until prompt 14), `lanekeeper_web_cert_reload_failures_total` rising,
`lanekeeper_web_cert_not_after_seconds` within a week of now, the audit chain's verification-failure counters (the series `hub_identity`
names), `/readyz` failing on one replica for longer than a rollout.
