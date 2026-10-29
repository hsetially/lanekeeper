# The kind end-to-end test

`run.sh` installs the database chart and the hub chart on a kind cluster of one control plane and three workers, with CloudNativePG, checks
S6 on the running database, and asserts what the **production** hub image can do in kind (plan 09, acceptance 3; see "Where the hub stage
stands"). It builds nothing: it takes the hub image that `images.yml` built and scanned. CI runs it as the `kind-e2e` job of `images.yml`;
locally, `just kind-e2e` (it needs Docker on a cgroup v2 host, kind, helm and crane at the versions of the Justfile).

**What it does not measure, and says so in every summary: `/readyz` on 3 replicas and P11 on the pod are NOT MEASURED in kind (the production
image cannot be Ready without Google and Entra), acceptance 3 is not met by this job.** The sentence is `KIND_NOT_MEASURED` in `lib.sh`; `run.sh`
puts it in the summary and the workflow appends it to the job summary. Nothing in this repository ticks acceptance 3 because this job is green.

| File | What it is |
|---|---|
| `run.sh` | the test: stages `registry`, `cluster`, `hub image`, `operator`, `database`, `s6`, `hub`, each timed; the first failure stops it with the stage named. `--registry-only` runs `registry` and `hub image` without a cluster |
| `lib.sh` | what needs no cluster, so that `.github/scripts/kind-run.test.mjs` can test it: the P11 verdict, the registry helpers (`wait_for_http`, `registry_report`, `registry_container_ip`), the verdict on the hub replicas (`hub_log_usable`, `hub_replica_class`, `hub_verdict`) and, in one block, every string it looks for in a hub log (`HUB_*_MARKERS`) |
| `registry-test.sh` | `run.sh --registry-only` and the registry helpers against a real registry container, crane and Docker (`just kind-registry-test`, part of `verify-09`, run by the `policies and scripts` job) |
| `pins.env` | what the test installs, each pin read from a registry or a release (never guessed) and at least 14 days old |

## The registry, and what the first CI run showed

The first run on GitHub (run 40366344156) created the cluster (50 s) and then failed in `hub image` after 0 s: `crane` got `connection refused` from
`[::1]:32768` for `https://localhost:32768/v2/` and for `http://localhost:32768/v2/`. The registry had answered on `127.0.0.1:32768` when it
was started (the `cluster` stage waited for it and passed), so a missing wait at the start was not the cause. What the log proves is that
the one route the push used, the host port that Docker published, no longer answered 46 s later, and that nothing in the output said what
the container was doing then. The script now:

- starts the registry in its own stage and waits for `/v2/` within `LK_KIND_REGISTRY_WAIT` seconds (60), one ask per second;
- asks again before the push, on **every route** (`127.0.0.1:<published port>` first, then the container's address on the kind network, port 80),
  for up to the same bound, and pushes to the first that answers. It reaches the registry by `127.0.0.1`, never `localhost`, which a runner
  resolves to `::1` first. When the published port is not the route that answers, the output says so (`NOTE: ...`);
- probes both routes right after the registry joins the kind network and again when a stage fails, so the next run shows which step took a route away;
- on a failure of the `registry`, `cluster` or `hub image` stage, prints the container's status, exit code and OOM flag, the published ports, its
  networks and addresses, the last 30 lines of its log and the host's listening sockets on the port; the same goes to `diagnostics.txt`;
- names any command that fails with no reason of its own (`a command failed with exit N at run.sh:LINE ...`), through the ERR trap.

**The third run answered it** (run 40374410436). The probe that `run.sh` now makes right after the registry joins the kind network printed
`http://127.0.0.1:32768/v2/ does NOT answer (after the registry joined the kind network): ... Couldn't connect to server` and
`http://172.18.0.6:80/v2/ answers`. So joining the kind network takes the published loopback port away on the runner, the container is alive, and the
second route (its address on the kind network) carried the push: `NOTE: the published port ... does not answer any more`, `hub image` passed in 2 s. Why
Docker does it was not investigated. Where a container's address is not reachable from the host (Docker Desktop's VM), neither route would answer after
the join; pushing before the join would be the order that works there, and it was not needed on the runner, which is the target.

The `cluster` stage also waits (`LK_KIND_NODES_TIMEOUT`, 180 s) for all four nodes to be Ready: `kind create --wait` waits for the control
plane only, and the workers were still `NotReady` when the first run printed them.

## Where the hub stage stands

**The decision (2026-10-28).** The production hub cannot become Ready in kind: at start it reads the web certificate and both CA certificates from
Secret Manager, signs with Cloud KMS (both CAs), and signs users in with Entra, and kind has none of them. That is by design (prompt 18, G23: no
fake in a release build, no `hub_dev` in the image, no setting that points the image at a stub). So the `hub` stage asserts what that image can
do, and nothing else is claimed:

1. **The migration Job completes** (`hub migrate`, exit 0) against the CloudNativePG database: TLS 1.3 with the server verified against the mounted
   `lanekeeper-db-ca`, the `uri` key of `lanekeeper-db-migrator`, the extensions, all migrations. It is a Helm pre-install hook, so
   `helm upgrade --install` waits for it (`--timeout`, `LK_KIND_JOBS_TIMEOUT`, 300 s) and fails when it fails; `run.sh` then waits with a bounded
   `kubectl wait --for=condition=complete` and checks `.status.succeeded`, and prints the Job's state and log when it did not.
2. **The seed Job completes** (`hub seed`, as `lanekeeper_app`, from the synthetic file of `deploy/values/hub/kind.yaml`). It is found by its labels
   (its name is a hash) and waited for the same way.
3. **Each of the 3 replicas reaches its own refusal at the first Google step**, which is what the log and the exit code show:
   - the container ended with **exit code 1**, never 2: the 25 required settings were read and accepted (`settings.require_serving()` runs first,
     and a refused setting is exit 2);
   - its log holds the hub's typed start error for a **Google** component: `the web certificate could not be loaded (LK_WEB_CERT_REF,
     LK_WEB_KEY_REF)`, a CA whose certificate secret could not be read or whose set-up took too long, or `starting took too long: web certificate`.
     kind has no metadata server, so the read may fail or run to `LK_START_TIMEOUT_SECS` (8 s); both are Google's, and either is accepted;
   - its log holds **no setting error** (a line `hub: ...`, or exit 2), **no database error** and **no schema error**. The database and the schema
     checks run in parallel with the Google reads and each runs to its end, and every failure is logged (`load` in `crates/hub/src/process.rs`), so a
     log that has only a Google error shows that the hub reached the database over TLS with full verification, as `lanekeeper_app`, and that the
     schema the Job made is the one the hub expects.

**Every other state fails the stage, and the failure says what was seen** (`hub_replica_class` in `lib.sh`; `kind-run.test.mjs` runs the real
`run.sh` against a stand-in kubectl for each):

| Class | What it is | Result |
|---|---|---|
| `google` | the above, on all 3 replicas | pass |
| `setting` | exit code 2, or a line `hub: ...` (a setting or the command line was refused, or the process stopped before its start) | fail, the line quoted |
| `schema` | `the database schema is not usable`: the migration did not run, or not against this database | fail |
| `database` | `the database is not usable`, or `starting took too long: database` | fail |
| `ready` | a pod is Ready. The production image cannot be, in kind: an unexpected state for a person to read (P11 is measured and printed if all 3 are) | fail |
| `wrong-exit` | a Google error in the log and an exit code that is not 1 (or none: the container has not ended) | fail |
| `unknown` | a usable log with none of the errors above: printed in full | fail |
| `unlogged` | no log could be retrieved in `LK_KIND_LOG_WAIT` seconds (30); its state is printed. Nothing is asserted without a log | fail |

A failure beside a Google error is the failure (the order is setting, schema, database, Google). Each replica's log is that of its current and its
previous container, whichever are usable: `kubectl logs` answers with exit 0 and the sentence `unable to retrieve container logs for
containerd://...` in place of a log when the kubelet has removed the container (the third run took that sentence for a log), and `hub_log_usable`
refuses it. Each replica's state (restart count, ready, waiting reason, exit code, started, finished) is printed.

**The one place that couples the script to prompt 18's wording** is the block of `HUB_*_MARKERS` in `lib.sh`. Each marker is a literal of the
non-test Rust source of `crates/hub/src` (the `Display` of `StartError`, `CaStep` and `PlatformError`), looked for with `grep -F`.
`kind-run.test.mjs` reads that source, extracts its string literals (leaving out comments and `#[cfg(test)]` modules) and fails when a marker is
no longer one, so a reworded message turns this job red in the pull request that rewords it. It also fails when `lib.sh` gains a marker the test
has no anchor for.

**Not run.** The stage itself has not run: the sandbox where it was written has no Docker daemon and a cgroup v1 kernel (kind's node image refuses
it). What was shown is the script against stand-ins (`.github/scripts/kind-run.test.mjs`: the real `run.sh` and `lib.sh` with a fake docker, kind, helm,
curl and crane, so a fake kubectl behind `docker exec`), `shellcheck -x`, `bash -n` and `actionlint`. The exact text of the first Google error in
kind (a failed read, or `starting took too long: web certificate`) is a guess from the code until a runner has shown it; the markers accept both.

- **Readiness.** A pod's `Ready` condition is the answer of `/readyz` on the operations port, as the kubelet's readinessProbe sees it. The
  test does not ask the port again from outside the pod: the hub chart's NetworkPolicy admits 9090 only from the monitoring namespace and
  Google's ranges, so a request through the API server's pod proxy would be refused by the very policy the chart ships. No replica is Ready here, so
  `/readyz` is not measured.
- **P11.** `perf/budgets.toml` `P11` is 10000 ms per replica. The hub's start-up with software edges is measured by the benchmark `hub/startup`
  (`cargo xtask bench-check`); what a cluster adds is probe timing and real Google latency, and kind cannot show either. `p11_verdict` in `lib.sh`
  is used only in the `ready` class above. If it ever runs, the number includes the probes' own timing (the startupProbe's period of 2 s,
  `deploy/charts/hub/templates/statefulset.yaml:96-100`, then the readinessProbe's 5 s, lines 106 to 108, and the API's one-second timestamps: up
  to about 7 s of the 10 s in the worst case, from the periods), so a miss is read against those before it is read as a slow start.
- **Proven on a runner (run 40374410436):** the stages `registry`, `cluster`, `hub image`, `operator`, `database` and `s6` pass. The `s6` stage checked what
  the plan says: a TLS connection works and is TLSv1.3 (the positive control), `psql sslmode=disable` is refused with `pg_hba.conf rejects connection for
  host ..., no encryption` (the chart's `hostnossl ... reject`, not a missing entry or a wrong password), and a client that offers TLS 1.2 at most is
  refused with `SSL error: tlsv1 alert protocol version`; then `vector` and `pg_trgm` and the collation. The `database` stage took 97 s of the 900 s that
  `LK_KIND_DB_TIMEOUT` allows (three instances, with their image pulled), so a runner several times slower still fits.
- **Unproven.** The `hub` stage as written (above: it has not run); that the CNI of kind (kindnet) enforces the hub's NetworkPolicy, and that the
  kubelet's probes are not filtered by it; `/readyz` on 3 replicas, P11 on the pod, the PodDisruptionBudget, a rolling update, the SIGTERM drain,
  Workload Identity, the Gateway and Cloud Armor (none is possible in kind with this image).

## The pins, and who moves them

`renovate.json` reads `pins.env` (and `deploy/images/tools.env`) with custom managers. What Renovate can express, it sees; what it cannot, it
cannot compute, and a person does.

| Pin | Renovate | A person |
|---|---|---|
| `LK_KIND_NODE_IMAGE` (`kindest/node`, tag and digest) | proposes a newer tag with its digest, after 14 days, in the group "docker images and digests" | checks that the kind binary of the Justfile supports the Kubernetes version; the test is the check |
| `LK_CNPG_OPERATOR_IMAGE` (tag and digest) | sees a new release; **the update waits for approval on the dependency dashboard**, in the group "cloudnative-pg operator" | see below |
| `LK_CNPG_VERSION` | sees a new GitHub release of `cloudnative-pg/cloudnative-pg` (tags are `v1.2.3`, the pin is `1.2.3`), in the same group | see below |
| `LK_CNPG_MANIFEST_URL` | rebuilt whole when the version moves (the version is in it twice: the tag and the file name) | see below |
| `LK_CNPG_MANIFEST_SHA256` | **cannot compute it**: no Renovate datasource hashes a release asset | `curl -fsSL "$LK_CNPG_MANIFEST_URL" \| sha256sum`; `run.sh` refuses a manifest whose hash differs, so a forgotten update fails the `operator` stage and applies nothing |

A CloudNativePG update is a person's even after Renovate has opened it: the new manifest's sha256 (above); the CRDs vendored in
`deploy/schemas` (their lock, `just schemas-verify`); and the version that `deploy/charts/db/README.md` names, which
`.github/scripts/consistency.test.mjs` checks against `pins.env`. That is why the rule waits for approval instead of opening a pull request
that is red by construction.

The tools that run the test (kind, helm, crane, cosign, ...) are pinned in the Justfile by version and sha256 of the release file.
Renovate does not read the Justfile, so those are moved by hand (they were not covered before this directory was either).
