# hub

The Lanekeeper hub: one binary that serves the web app, the API, the live stream, the MCP server, the Git webhook and the blob
endpoint on the web listener, and the agent and replica gRPC services on their own mTLS listeners. The contract for the pod and
the network it runs in is `HARDENING.md`; the design is `plans/18-hub-assembly.md`.

This file says how to operate it. It grows with the tasks of prompt 18.

## Commands

```
hub [serve]    run the hub until SIGTERM or SIGINT (the default)
hub migrate    apply the database migrations as lanekeeper_migrator, then exit
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
