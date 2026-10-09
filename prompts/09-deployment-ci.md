# 09: Deployment, CI and supply chain

| | |
|---|---|
| **Wave** | 1 |
| **Depends on** | 01 |
| **You own** | `deploy/**`, `.github/**`, `renovate.json`, Dockerfiles, the deployment and supply-chain section of `docs/threat-model.md` |
| **Interfaces** | Runtime configuration for every crate, set through Helm values |
| **Security** | S6, S7, S10, S16, S17, S18, S19, S20 |
| **Performance** | P11 (resources), plus support for P2 (webhook route) |
| **Gate** | `just verify-09` |

## Objective

Reproducible, signed and verified builds, deployed hardened by default, with highly available topology and least-privilege networking.

## Tasks

### T1: Images (S16, S20)

**Dockerfiles:**

- multi-stage, using cargo-chef;
- builds with `--locked`;
- base images pinned by digest;
- the build stage includes a C compiler, which tree-sitter needs;
- distroless runtime images with a non-root uid.

**Images:**

- **`hub`:** includes `web/dist` and the pinned embedding model files.
- **`agent`.**

**Per image, in CI:**

- a CycloneDX SBOM;
- an image vulnerability scan that fails on critical findings.

**Verify:** images build reproducibly; two builds of the same commit produce the same digest, or any difference is documented.

### T2: CI (S19, S20)

**Workflows** with path filters:

- **`rust`:** fmt; clippy; tests with a Postgres service; `sqlx prepare --check`; `cargo deny`; `cargo audit`; `just fuzz-smoke`.
- **`web`:** lint; typecheck; tests; bundle gate; Lighthouse CI; `osv-scanner`.
- **`contracts`:** `buf`; `redocly`; migration and role tests.
- **`bench`:** criterion benchmarks plus `bench-check`, on a fixed-size runner, comparing against `main`.
- **`images`:** build; SBOM; scan; push to GHCR with `GITHUB_TOKEN`; keyless cosign signing; provenance attestation.
- **`nightly`:** fuzzing for 10 minutes per target; the full benchmark suite at target scale.

**Rules:**

- every action pinned by commit SHA;
- minimal `permissions:` per job;
- OIDC to GCP through Workload Identity Federation, with no stored keys.

**Renovate:** `minimumReleaseAge` of 14 days, grouped updates, and automerge disabled for the contract crates.

**Verify:** `actionlint` passes, and a test fails if any action reference isn't a SHA.

### T3: Hub chart (D62, S16, S18)

**Topology:**

- a StatefulSet of 3 replicas, with a persistent volume per pod for the Git mirror;
- pod anti-affinity across zones;
- a PodDisruptionBudget with `minAvailable: 2`;
- a headless service for routing between replicas.

**Security context:**

- read-only root filesystem;
- seccomp RuntimeDefault;
- all capabilities dropped;
- non-root.

**Identity and secrets:**

- Workload Identity for Secret Manager, KMS, GCS and Cloud Trace;
- no Kubernetes Secrets apart from CloudNativePG's database credentials.

**Load balancers:**

- **Agents:** a TCP pass-through `LoadBalancer` with `loadBalancerSourceRanges` set to the clusters' outbound IPs (Q1).
- **Users:** a Gateway with TLS and a Cloud Armor policy allowing the GlobalProtect ranges (Q2). The `/hooks/github` route additionally allows GitHub's hook ranges (Q28).

**NetworkPolicies:**

- default-deny;
- egress only to Entra, GitHub, Google APIs, the Teams webhook host, Postgres, and the hub pods themselves.

**Verify:** kubeconform passes, and a policy test rejects pods missing any hardening field.

### T4: Agent chart (S16, S17)

- One replica.
- The NFS volume and the uid/gid from values (Q11).
- Workload Identity annotation (Q26), with a join-token fallback Secret only when that's enabled.
- RBAC from `crates/agent/RBAC.md`.
- An egress NetworkPolicy allowing only the hub endpoint and the metadata server.
- The same hardening as the hub.

**Verify:** kubeconform, plus a test that the Role contains no Secret list.

### T5: Data and storage

**CloudNativePG:**

- 3 instances, TLS required;
- backups to a CMEK-encrypted GCS bucket (Q29), kept for 30 days;
- the `lanekeeper_migrator` and `lanekeeper_app` roles;
- pgvector and pg_trgm available in the image. Build a custom image if needed, pinned by digest.

**GCS buckets:**

- the audit checkpoint bucket, with a locked retention policy (S9);
- the docs bucket, with versioning on.

Both use uniform access, CMEK and no public access.

### T6: Admission and operations

- A Binary Authorization policy that requires cosign signatures from your CI identity (Q24).
- ArgoCD Applications: the hub in the platform cluster, and agents per swimlane, each with its own values file.
- `deploy/ADDING-A-SWIMLANE.md`, covering these steps:
  1. Create the swimlane.
  2. Set up Workload Identity, or a join token as the fallback.
  3. Add the values file.
  4. Add the ArgoCD Application.
  5. Add the cluster's outbound IP to the allowlist.
  6. Run the adoption pass.

**Verify:** `helm template` with every values file, and conftest policies for hardening and network rules.

### T7: Threat model

Fill in the deployment and supply-chain section of `docs/threat-model.md`.

### T8: Agent spool volume and sentinel packaging

- **Agent spool:** the agent chart gets a 1 GiB persistent volume for the spool (D74). The Deployment uses the `Recreate` strategy.
- **Sentinel build:**
  - a static `x86_64-unknown-linux-musl` binary, built in CI;
  - packaged as an RPM with a hardened systemd unit (S25), the audisp af_unix plugin configuration, and the auditd watch rule template;
  - RPM-signed and cosign-signed, with an SBOM.

**Verify:**

- the RPM installs and starts on Rocky Linux 8.9 and 8.10 containers;
- `systemd-analyze security` gives an exposure score of 2.0 or lower;
- the RPM's signature verifies.

## Acceptance (all required)

- [ ] `just verify-09` passes: lint, kubeconform, conftest and the SHA-pin test.
- [ ] Images are signed, and their SBOMs and provenance attestations are published.
- [ ] A kind cluster in CI runs the hub chart with CloudNativePG, and `/readyz` passes on all 3 replicas.

## Stop and escalate if

- Binary Authorization, CMEK or Workload Identity isn't available (Q24, Q26 or Q29). Configure the fallbacks and flag them.

## Out of scope

Application code.
