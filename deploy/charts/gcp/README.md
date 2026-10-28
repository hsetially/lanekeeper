# gcp chart

The Google Cloud resources of Lanekeeper as Config Connector manifests (plan 09 A4). **09/T3 starts it with Cloud Armor;** T5 adds
the buckets, the service accounts and the IAM around them, and **T6 adds Binary Authorization** (admission, below). Secret containers only,
never values.

`templates/cloudarmor.yaml` renders `ComputeSecurityPolicy/<cloudArmor.policyName>` (the hub chart's `gateway.securityPolicyName`):

| Priority | Allows | Rule |
|---|---|---|
| 1000.. | the users' GlobalProtect egress ranges (Q2), any path | `SRC_IPS_V1`, ten ranges a rule |
| 2000.. | GitHub's webhook ranges (Q28), the exact path `/hooks/github` and nothing else | `request.path == '/hooks/github' && (inIpRange(origin.ip, '...') \|\| ...)`, four ranges a rule |
| 2147483647 | nobody | `deny(403)` |

`deploy/policy/network.rego` checks the rendering: the hooks expression must be exactly that shape, GitHub's ranges may appear in no
other rule, every range GitHub lists must be in a hooks rule, no allowed range may be the internet, and the default must deny.

## GitHub's ranges are generated, never typed

`deploy/values/hub/github-hook-ranges.yaml` is written by `node deploy/tools/github-hook-ranges.mjs --write` from the `hooks` list of
`https://api.github.com/meta` and layered over the values (`-f github-hook-ranges.yaml -f prod.yaml`). The nightly job runs
`--check`. **Until a person runs `--write` with network access the file is empty and the chart refuses to render**, which is how
the missing step shows (plan 09 blockers, B5).

## Buckets, service accounts and IAM (09/T5)

| Renders | Why |
|---|---|
| `StorageBucket` x 3: backup, audit checkpoint, docs | uniform access, `publicAccessPrevention: enforced`, a customer-managed key (S10), and `cnrm.cloud.google.com/deletion-policy: abandon` with `argocd.argoproj.io/sync-options: Prune=false`, so that removing or pruning the resource never deletes a bucket |
| the audit checkpoint bucket's `retentionPolicy` | a required period (no default: it is a person's decision, plan 09 A19; prod's is three months, 7,776,000 s, decided by a human on 2026-10-26), **locked in prod only**, because a lock cannot be undone (S9) |
| the docs bucket's `versioning` | on |
| `IAMServiceAccount` `lanekeeper-hub` and `lanekeeper-db` | what the two run as; the hub chart's `serviceAccount.gcpServiceAccount` and the db chart's are these |
| `IAMPolicyMember` Workload Identity x 2 | `PROJECT.svc.id.goog[NAMESPACE/NAME]` may act as the Google account (S5, S7) |
| `IAMPolicyMember` for the buckets | the database: `objectUser` on the backups, nothing else; the hub: `objectCreator` and `objectViewer` on the audit checkpoints (it can neither delete nor replace one), `objectUser` on the docs; the hub also gets `cloudtrace.agent` on the project |
| `IAMPolicyMember` `cryptoKeyEncrypterDecrypter` | on each key a bucket uses, for the Cloud Storage service agent; on each key in `cmek.diskKeys`, for the Compute Engine service agent. Both are named by `projectNumber` |

`deploy/policy/storage.rego` checks the buckets and `iam.rego` every binding: service accounts only, a short list of roles, each on
the one resource kind it is for, and each bucket for its own caller. A person, a group, `allUsers`, a basic role and a role on the whole
project (but Cloud Trace) are refused.

**The keys are not made here.** A bucket and a disk take the name of a Cloud KMS key that exists (Q29 has not chosen the key ring), in
the bucket's `location`. Without a key a bucket falls back to Google-managed encryption, which renders with a warning that
`render.sh` makes a failure for prod and staging. The signing keys (the agent CA, the replica CA, the audit checkpoint key, the token
KEK) and the hub's Secret Manager and KMS bindings are **not in this commit**: their names are settings of the hub chart (`config.kms.*`
and the Secret Manager names of `crates/hub/HARDENING.md`), and nothing here creates what they name.

## Admission: only images signed by CI run (09/T6; S19, S20, plan 09 A5)

`templates/binauthz.yaml` renders three Config Connector resources, in this order of dependence:

| Renders | Why |
|---|---|
| `ContainerAnalysisNote/<attestorName>` | the place an attestation about an image is written; an attestation authority (`attestation.hint.humanReadableName`) |
| `BinaryAuthorizationAttestor/<attestorName>` | trusts the **public key of the CI signing key in Cloud KMS** (`binaryAuthorization.publicKeys`), and reads attestations from that note. A key that cannot be exported, never one kept somewhere: the key `id` must be a Cloud KMS key version, and the PEM must be a public key |
| `BinaryAuthorizationPolicy/lanekeeper` | the project's one admission policy. The default rule is `REQUIRE_ATTESTATION` by that attestor, `globalPolicyEvaluationMode: ENABLE` so that Google's system images are let through by Google's policy, and **no other rule** (a rule for a cluster or a namespace overrides the default) |

None of the three is deleted by removing it from Git or pruned by a sync (`deletion-policy: abandon`, `Prune=false`): deleting the
policy resets the project to the default policy, which (as far as the authors know; Google's documentation could not be read, B6) admits every image.

**Dry run is the default (plan 09 A5, Q24).** `binaryAuthorization.enforcementMode` is `DRYRUN_AUDIT_LOG_ONLY` until Q24 (is Binary
Authorization available on the platform cluster and the swimlane clusters?) is answered: an image without an attestation is written to
the audit log as one that would have been blocked, and runs. `deploy/policy/binauthz.rego` says so on every run (`WARN ... is in
dry-run`), and allows it in every environment. Setting `ENFORCED_BLOCK_AND_AUDIT_LOG` is the one-line change that blocks, made in the
environment's values by a person. **Before that**, read the dry run's audit log: every image in the cluster that is not ours and not
Google's (the CloudNativePG operator and instances, Argo CD, Config Connector) would be blocked, and needs an attestation of ours or a
name pattern in `binaryAuthorization.admissionWhitelistPatterns`. The policy refuses a pattern that covers an image of ours (`ghcr.io/*`,
`ghcr.io/hsetially/lanekeeper/*`).

**The public key is not in the committed values, and the chart refuses to render without it.** The CI signing key is made in Cloud
KMS by a person (an asymmetric signing key, `EC_SIGN_P256_SHA256` for example, in the project and location of the environment; the
`images.yml` job `publish` signs with it through Workload Identity Federation when the repository variable `LK_COSIGN_KMS_KEY` is
set), and its public half is put into the environment's values:

```
gcloud kms keys versions get-public-key 1 --key=<key> --keyring=<ring> --location=<location> --project=<project> > public.pem
```

```yaml
binaryAuthorization:
  publicKeys:
    - keyVersion: projects/<project>/locations/<location>/keyRings/<ring>/cryptoKeys/<key>/cryptoKeyVersions/1
      signatureAlgorithm: ECDSA_P256_SHA256
      publicKeyPem: |
        -----BEGIN PUBLIC KEY-----
        ...
        -----END PUBLIC KEY-----
```

Rotation adds a version to the list, signs with it, and then removes the old one. Until a person does this, the missing key shows as
the chart's refusal, the way the missing GitHub ranges do. `deploy/tests/render.sh` stands in a throwaway public key
(`deploy/tests/testdata/synthetic-attestor-key.yaml`, SYNTHETIC: its private half never existed on disk) so that the policies and the
planted cases can run, and only while the environment's values have no key; once a person has put the real one in, that key is the one
that is checked.

**What this chart does not do, and has not verified.** The CI signing key itself, the Workload Identity Federation pool and provider
that GitHub's OIDC token signs in through, and the IAM that lets only this repository's CI sign with the key are **not made here**
(plan 09 A4 lists them among the resources of this chart; T3, T5 and T6 do not make them): the platform team makes them, and the
`images.yml` variables name them. The project's two Google APIs (`binaryauthorization.googleapis.com` and
`containeranalysis.googleapis.com`) must be enabled. The policy is **per project**, so this one protects the platform project's
clusters; a swimlane cluster in another project needs a policy there that requires the same attestor, which this chart does not make
(Q24 asks whether Binary Authorization is available there at all). And Google's documentation (`docs.cloud.google.com`) could not be
read in the sandbox that wrote this (plan 09 blockers, B6), so these are taken from the Config Connector CRDs (v1.159.0) and its
samples, not from the product's documentation: that a **cosign signature** made with the KMS key is what a classic attestor accepts
(the attestor reads attestations from Container Analysis, and `images.yml` writes a signature to the registry), that the key `id` has
the form `//cloudkms.googleapis.com/v1/<key version>`, and that Binary Authorization's service account needs no IAM binding on the
note that this chart does not make. The dry run is how the first apply finds out. Config Connector v1.159.0 also has
`BinaryAuthorizationPlatformPolicy` (alpha), whose `sigstoreSignatureCheck` takes a cosign public key directly; it is not a drop-in (its spec
has no `enforcementMode` field, so not the dry-run default of plan 09 A5), and it is the alternative to ask about if the attestor cannot see
cosign signatures.

### Checked without Cloud Armor

Everything in the sections above but Cloud Armor is checked on its own, with `--set cloudArmor.enabled=false` (`deploy/tests/render.sh`
calls this render `gcp-storage`; `just deploy-data` runs it). That is an addition to the checks of the whole chart, not a replacement for
them: with the committed values the chart still refuses to render until GitHub's ranges exist (B5, above), and `render.sh` checks that the
isolated render has no Cloud Armor policy and that no committed values file turns Cloud Armor off.
