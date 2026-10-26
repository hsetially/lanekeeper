# The kind end-to-end test

`run.sh` installs the hub chart and the database chart on a kind cluster of one control plane and three workers, with CloudNativePG, and
checks S6 on the running database and P11 on the hub (plan 09, acceptance 3). It builds nothing: it takes the hub image that `images.yml`
built and scanned. CI runs it as the `kind-e2e` job of `images.yml`; locally, `just kind-e2e` (it needs Docker on a cgroup v2 host, kind,
helm and crane at the versions of the Justfile).

| File | What it is |
|---|---|
| `run.sh` | the test: stages `registry`, `cluster`, `hub image`, `operator`, `database`, `s6`, `hub`, each timed; the first failure stops it with the stage named. `--registry-only` runs `registry` and `hub image` without a cluster |
| `lib.sh` | what needs no cluster, so that `.github/scripts/kind-run.test.mjs` can test it: the P11 verdict, and the registry helpers (`wait_for_http`, `registry_report`, `registry_container_ip`) |
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

What is **not** known, and the script does not claim to know, is why the published port stopped answering (a registry that died, or a
mapping that Docker dropped when the registry joined the kind network, are both possible, and the log cannot tell them apart). The second
route is the registry's address on the Docker network, which a Linux host reaches directly (the runner is one; Docker Desktop's VM is not,
and there the published port is the only route). The next run's output decides it.

The `cluster` stage also waits (`LK_KIND_NODES_TIMEOUT`, 180 s) for all four nodes to be Ready: `kind create --wait` waits for the control
plane only, and the workers were still `NotReady` when the first run printed them.

## Where the hub stage stands

- **A1.** `crates/hub/src/main.rs` exits 1 by design until the identity module (03a) and the registry (05) are composed into the binary, so
  the three replicas start, refuse and crash, and the `hub` stage fails with that reason. The script reports it as such (it counts the
  "refusing to start without them" lines of all three replicas) and fakes nothing. The summary then says **P11 NOT MEASURED**.
- **Readiness.** A pod's `Ready` condition is the answer of `/readyz` on the operations port, as the kubelet's readinessProbe sees it. The
  test does not ask the port again from outside the pod: the hub chart's NetworkPolicy admits 9090 only from the monitoring namespace and
  Google's ranges, so a request through the API server's pod proxy would be refused by the very policy the chart ships.
- **P11.** Once all three replicas are Ready, each must be Ready within **10 s** of its container's start (`perf/budgets.toml` `P11`, 10000 ms;
  `p11_verdict` in `lib.sh`). The number includes the probes' own timing, not only the hub's start: the kubelet runs no readinessProbe until the
  startupProbe has succeeded, so the time to Ready can hold up to the startupProbe's period (2 s, `deploy/charts/hub/templates/statefulset.yaml:96-100`)
  and then up to the readinessProbe's (5 s, the same file, lines 106 to 108), and the API's one-second timestamps: up to about 7 s of the 10 s in the
  worst case, from the periods and not measured (the test has never run). A replica that passes started within the budget; **a red P11 on kind may be
  probe overhead, so read it against those periods before reading it as a slow start.** The budget is the design scale's, and kind's CPUs and disks are not: a miss is a finding to read, not
  a flake to retry.
- **Unproven.** That the CNI of kind (kindnet) enforces the hub's NetworkPolicy, and that the kubelet's probes are not filtered by it. No kind
  cluster could start where this was written (cgroup v1, and `pkg-containers.githubusercontent.com` refused: blockers B16 and B9).

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
