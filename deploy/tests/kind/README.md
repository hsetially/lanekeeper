# The kind end-to-end test

`run.sh` installs the hub chart and the database chart on a kind cluster of one control plane and three workers, with CloudNativePG, and
checks S6 on the running database and P11 on the hub (plan 09, acceptance 3). It builds nothing: it takes the hub image that `images.yml`
built and scanned. CI runs it as the `kind-e2e` job of `images.yml`; locally, `just kind-e2e` (it needs Docker on a cgroup v2 host, kind,
helm and crane at the versions of the Justfile).

| File | What it is |
|---|---|
| `run.sh` | the test: stages `cluster`, `hub image`, `operator`, `database`, `s6`, `hub`, each timed; the first failure stops it with the stage named |
| `lib.sh` | what needs no cluster, so that `.github/scripts/kind-run.test.mjs` can test it: the P11 verdict |
| `pins.env` | what the test installs, each pin read from a registry or a release (never guessed) and at least 14 days old |

## Where the hub stage stands

- **A1.** `crates/hub/src/main.rs` exits 1 by design until the identity module (03a) and the registry (05) are composed into the binary, so
  the three replicas start, refuse and crash, and the `hub` stage fails with that reason. The script reports it as such (it counts the
  "refusing to start without them" lines of all three replicas) and fakes nothing. The summary then says **P11 NOT MEASURED**.
- **Readiness.** A pod's `Ready` condition is the answer of `/readyz` on the operations port, as the kubelet's readinessProbe sees it. The
  test does not ask the port again from outside the pod: the hub chart's NetworkPolicy admits 9090 only from the monitoring namespace and
  Google's ranges, so a request through the API server's pod proxy would be refused by the very policy the chart ships.
- **P11.** Once all three replicas are Ready, each must be Ready within **10 s** of its container's start (`perf/budgets.toml` `P11`, 10000 ms;
  `p11_verdict` in `lib.sh`). The number includes the readinessProbe's period (5 s at most) and the API's one-second timestamps, so a replica
  that passes started within the budget. The budget is the design scale's, and kind's CPUs and disks are not: a miss is a finding to read, not
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
