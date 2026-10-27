# hub

The Lanekeeper hub: one binary that serves the web app, the API, the live stream, the MCP server, the Git webhook and the blob
endpoint on the web listener, and the agent and replica gRPC services on their own mTLS listeners. The contract for the pod and
the network it runs in is `HARDENING.md`; the design is `plans/18-hub-assembly.md`.

This file says how to operate it. It grows with the tasks of prompt 18.

## Commands

```
hub [serve]    run the hub until SIGTERM or SIGINT (the default)
hub migrate    apply the database migrations as lanekeeper_migrator, then exit
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
