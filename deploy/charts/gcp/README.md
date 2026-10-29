# gcp chart

The Google Cloud resources of Lanekeeper as Config Connector manifests (plan 09 A4). **09/T3 starts it with Cloud Armor;** T5 adds
the buckets, the service accounts and the IAM around them, **T6 adds Binary Authorization** (admission, below), and **09/fix (gap I1) adds
the hub's Cloud KMS keys, the Secret Manager containers it reads and the IAM on each** ("The hub's keys and secrets", below). Secret
containers only, never values.

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

**The customer-managed keys of the buckets, the disks and the secrets are not made here.** A bucket, a disk and the secrets
(`secrets.cmekKey`) take the name of a Cloud KMS key that exists (Q29 has not chosen the key ring), in `location`. Without a key a
bucket or a secret falls back to Google-managed encryption, which renders with a warning that `render.sh` makes a failure for prod and
staging. The keys that the hub itself uses, and the secrets it reads, are made here: next section.

## The hub's keys and secrets (09/fix, I1; S5, S7, S8, S9)

`templates/kms.yaml`, `secrets.yaml` and `iam.yaml` render what `crates/hub/HARDENING.md` says the hub's Google service account needs
(the names are the hub chart's, `render.sh contract` compares them). `deploy/policy/kms.rego` checks the keys and the secrets, and
`iam.rego` every binding on them.

| Renders | Why |
|---|---|
| `KMSKeyRing/<kms.keyRing>` in `location` | the ring of the four keys |
| `KMSCryptoKey` agent CA, replica CA, audit checkpoint | `ASYMMETRIC_SIGN`, `EC_SIGN_P256_SHA256`, `kms.protectionLevel` (HSM; `kms.rego` requires it in prod). **Two separate CA keys** (an agent's certificate is never a replica's, S5), each used for nothing else. No rotation period: the hub pins a key *version*, so rotating one is a person's step; older versions stay enabled (the checkpoint key's must, to verify old checkpoints) |
| `KMSCryptoKey` vault | `ENCRYPT_DECRYPT`, `GOOGLE_SYMMETRIC_ENCRYPTION`: wraps the data key of each stored GitHub token (S8). The hub names the key and no version; `kms.vaultRotationPeriod` (90 days) gives it a new primary version now and then |
| all five | `deletion-policy: abandon` and `Prune=false`: removing them from Git or from Argo CD destroys nothing. `destroyScheduledDuration` is 30 days, so that a destroyed version can be restored in time. The key material is made by Cloud KMS: no `importOnly`, no `skipInitialVersionCreation`, and no key *version* is a resource of this chart |
| `SecretManagerSecret` x 10 | the containers the hub reads, never a value and never a version: `lanekeeper-web-cert`, `lanekeeper-web-key`, `lanekeeper-agent-ca-cert`, `lanekeeper-replica-ca-cert`, `lanekeeper-entra-client`, `lanekeeper-login-cookie` (the six `secrets.hub.*`, which default to the hub chart's `config.secretManager.*`), and `write-preview-key`, `github-read-token`, `github-webhook-secret`, `mcp-token-hmac-key` (constants of the hub's code, in the template). One replica in `location`, encrypted with `secrets.cmekKey`; `deletion-policy: abandon` and `Prune=false`. The spec is the replication and nothing else: no ttl, no expiry, no rotation, no topic |
| `IAMPolicyMember` `roles/cloudkms.signerVerifier` x 3 | the hub's account, on each of the three signing keys (on the key, never on the ring or the project) |
| `IAMPolicyMember` `roles/cloudkms.cryptoKeyEncrypterDecrypter` | the hub's account, on the vault key and on no other key |
| `IAMPolicyMember` `roles/secretmanager.secretAccessor` x 10 | the hub's account, one binding on each secret, never on the project |
| `IAMPolicyMember` `cryptoKeyEncrypterDecrypter` | Secret Manager's service agent (`service-<projectNumber>@gcp-sa-secretmanager.iam.gserviceaccount.com`), on `secrets.cmekKey` |

The hub's account gets nothing else on KMS or Secret Manager: not `cloudkms.admin`, not `signer` or `publicKeyViewer`, not a version
role, and not `secretAccessor` on a secret that is not in this output. `iam.rego` refuses each, and `render.sh planted` shows it.

### Which values feed which hub chart value

| This chart | Hub chart (`deploy/values/hub/<env>.yaml`) | Checked by `render.sh contract` |
|---|---|---|
| `project`, `location`, `kms.keyRing`, `kms.keys.agentCa` | `config.kms.agentCa` = `projects/<project>/locations/<location>/keyRings/<keyRing>/cryptoKeys/<agentCa>/cryptoKeyVersions/N` (`LK_AGENT_CA_KMS_REF`) | the key exists, is the agent CA key, and the hub's account may sign with it |
| `kms.keys.replicaCa` | `config.kms.replicaCa` (`LK_REPLICA_CA_KMS_REF`) | the same, and a different key |
| `kms.keys.auditCheckpoint` | `config.kms.auditCheckpoint` (`LK_AUDIT_CHECKPOINT_KMS_REF`) | the same |
| `kms.keys.vault` | `config.kms.vault`, **without a version** (`LK_VAULT_KMS_REF`) | the key exists, is the vault key, and the hub's account may encrypt and decrypt with it |
| `project` | `config.gcpProject` (`LK_GCP_PROJECT`) | every secret is made in that project |
| `secrets.hub.webCert`, `webKey`, `agentCaCert`, `replicaCaCert`, `entraClient`, `loginCookie` | `config.secretManager.*` (`LK_WEB_CERT_REF`, `LK_WEB_KEY_REF`, `LK_AGENT_CA_CERT_REF`, `LK_REPLICA_CA_CERT_REF`, `LK_ENTRA_CLIENT_AUTH_REF`, `LK_ENTRA_COOKIE_KEY_REF`) | each name has a container and a `secretAccessor` binding for the hub's account |
| (the template) `write-preview-key`, `github-read-token`, `github-webhook-secret`, `mcp-token-hmac-key` | none: constants read from `crates/hub-writes/src/config.rs`, `crates/hub-mcp/src/auth/pat.rs` and `crates/hub/src/config/git.rs` | the same, and a container that the hub does not read is a failure too |
| `buckets.auditCheckpoint.name` | `config.auditCheckpointBucket` (`LK_AUDIT_CHECKPOINT_BUCKET`) | as before (T5) |

A hub values file that renames a secret or a key without this chart's values following, or the other way round, fails `render.sh
contract` (it is a hub that cannot start, or a container nothing reads). The names of a secret are lower case letters, digits and
hyphens (a Kubernetes name); the hub chart allows more, and a name with a capital or an underscore is a change to this chart first.
**One project for each environment:** the names of the secrets are constants of the hub's code (`write-preview-key` cannot carry an
environment), so staging and prod are two projects. The keys are named for the environment (`lanekeeper-vault-prod`) all the same.

### What a person still does, in this order

None of it is automated or rendered, on purpose: values and key versions are what Git must never hold.

1. **Enable** `cloudkms.googleapis.com` and `secretmanager.googleapis.com` in the project, and make the customer-managed key of the
   secrets (`secrets.cmekKey`, a key of its own in `location`). Secret Manager's service agent must exist before the secrets can be
   written with that key: `gcloud beta services identity create --service=secretmanager.googleapis.com --project=<project>` is the
   authors' recollection of the command (**not verified**: the `beta` component could not be installed where this was written).
2. **Apply the chart** (Argo CD, project `lanekeeper-cloud`). Config Connector makes the ring, the four keys and the ten containers, and
   binds the roles.
3. **Read the key versions off the created keys.** The hub chart's `config.kms.agentCa`, `replicaCa` and `auditCheckpoint` name a key
   *version*, and a version's number is known only once the key exists:
   ```
   gcloud kms keys versions list --key=<kms.keys.agentCa> --keyring=<kms.keyRing> --location=<location> --project=<project> --format='value(name)'
   ```
   prints `projects/.../cryptoKeyVersions/N` for each version. That string is `LK_AGENT_CA_KMS_REF` (`config.kms.agentCa`), and the same for
   `kms.keys.replicaCa` (`LK_REPLICA_CA_KMS_REF`) and `kms.keys.auditCheckpoint` (`LK_AUDIT_CHECKPOINT_KMS_REF`). The vault key needs no
   version: `LK_VAULT_KMS_REF` is `projects/<project>/locations/<location>/keyRings/<kms.keyRing>/cryptoKeys/<kms.keys.vault>`, which the
   values already say. **The first version of each CA key must exist before `hub ca-init` and before the first rollout of the hub.** This
   chart sets no `skipInitialVersionCreation`, so that Cloud KMS makes it when it makes the key; that the version exists, and is number 1,
   is **not verified** (Google's documentation could not be read, plan 09 blockers B6): run the command above and use what it prints.
4. **Make the two CA certificates with `hub ca-init`** (`crates/hub/HARDENING.md`, "Making a CA certificate"): once for each CA key
   version, from the hub image, and store the output in `lanekeeper-agent-ca-cert` and `lanekeeper-replica-ca-cert`. The person who runs
   it needs `roles/cloudkms.publicKeyViewer` and `roles/cloudkms.signerVerifier` on **that one key** (not the ring, not the project; the
   hub documents the pair, as roles on a key *version*, and Cloud KMS grants IAM on the key that holds it, **not verified**). That grant
   is a person's, so this chart does not render it (`iam.rego` refuses a person as a member): add it for the run and remove it after,
   with `gcloud kms keys add-iam-policy-binding <key> --keyring=<ring> --location=<location> --member=user:<you> --role=<role>` and
   `remove-iam-policy-binding`.
5. **Put the values into the other eight secrets**, once each, with `printf %s "<value>" | gcloud secrets versions add <name> --data-file=-
   --project=<project>` (nothing but the value goes through the pipe, and shell history must not keep it):

   | Secret | The value |
   |---|---|
   | `lanekeeper-web-cert`, `lanekeeper-web-key` | the PEM chain and the PEM private key of the web listener's certificate (the hop from the load balancer to the pod) |
   | `lanekeeper-entra-client` | the Entra app registration's client secret |
   | `lanekeeper-login-cookie` | 32 random bytes as base64url |
   | `write-preview-key` | at least 32 bytes; the hub does not start without it |
   | `github-read-token` | the read-only GitHub credential (fine-grained) |
   | `github-webhook-secret` | the secret that GitHub also holds for the webhook |
   | `mcp-token-hmac-key` | at least 32 random bytes as base64url; read only with `LK_MCP_AUTH=personal_token`, and the hub does not start in that mode without it |
6. **Teams webhook secrets** are named at run time by an administrator (`notification_settings.destination_ref`), so this chart cannot bind
   them ahead of time and renders none. Each one the administrator names needs a container, a value and a binding for the hub's account,
   made by hand: `gcloud secrets add-iam-policy-binding <name> --member=serviceAccount:lanekeeper-hub@<project>.iam.gserviceaccount.com
   --role=roles/secretmanager.secretAccessor --project=<project>`. The install guide makes this a runbook step.

### Not verified

Google's documentation (`docs.cloud.google.com`) could not be read where this was written (plan 09 blockers, B6), so the following rest
on the Config Connector CRDs (v1.159.0), `crates/hub/HARDENING.md` and the authors' memory, and the first apply is the test:

- that Cloud KMS makes a first version when Config Connector makes the key, that it is number 1, and that **HSM keys with
  `EC_SIGN_P256_SHA256`** and `GOOGLE_SYMMETRIC_ENCRYPTION` are offered in `location` (if HSM is not, `kms.protectionLevel: SOFTWARE` is
  allowed in staging and refused in prod by `kms.rego`; relaxing prod is a decision for `docs/decisions.md`);
- that IAM on a key covers all its versions (so `signerVerifier` on the checkpoint key covers older versions), that a key ring and a key
  cannot be deleted once made, and what Config Connector does when it finds the ring or a key already there;
- that a version the vault key rotates away from still decrypts what it wrapped, and the allowed range of `destroyScheduledDuration`;
- that a secret with a single `userManaged` replica can be encrypted with a regional key in that region, and that a multi-region such as
  `eu` is not usable as `location` for the keys and the secrets;
- the name of Secret Manager's service agent and that the secrets' accessor needs no role on `secrets.cmekKey`;
- that `roles/cloudkms.signerVerifier` is enough for the hub to sign and to read the public key, as `HARDENING.md` says.

`LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS` (older checkpoint key versions that the nightly verification still accepts) is not a setting of the
hub chart (its `values.yaml` says why), so rotating the checkpoint key needs that setting first; this chart keeps the old versions
enabled and its role covers them.

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
