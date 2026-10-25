# gcp chart

The Google Cloud resources of Lanekeeper as Config Connector manifests (plan 09 A4). **09/T3 starts it with Cloud Armor;** T5 adds
the buckets, the service accounts and the IAM around them, T6 Binary Authorization. Secret containers only, never values.

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
| the audit checkpoint bucket's `retentionPolicy` | a required period (no default: it is a person's decision, plan 09 A19), **locked in prod only**, because a lock cannot be undone (S9) |
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
KEK) and the hub's Secret Manager and KMS bindings are **not in this commit**: their names are the hub's configuration, which is not
composed yet (plan 09 A1).

### Checked without Cloud Armor

Everything in this section is checked on its own, with `--set cloudArmor.enabled=false` (`deploy/tests/render.sh` calls this render
`gcp-storage`; `just deploy-data` runs it). That is an addition to the checks of the whole chart, not a replacement for them: with the
committed values the chart still refuses to render until GitHub's ranges exist (B5, above), and `render.sh` checks that the isolated
render has no Cloud Armor policy and that no committed values file turns Cloud Armor off.
