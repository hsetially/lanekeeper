# 10: Sync job reporting (investigation)

| | |
|---|---|
| **Wave** | Investigation, any time. Implementation waits for a human decision (D21). |
| **Depends on** | Access to a pilot cluster |
| **You own** | `docs/sync-job.md`. After the decision, the chosen integration, through a contract-change PR if needed. |
| **Interfaces** | Feeds `record_sync` in 05 |
| **Security** | S11, S17 (whatever option is chosen must not widen agent permissions without review) |
| **Gate** | A human reviews the recommendation |

## Objective

Document exactly how the sync Job works, then recommend how it should report completed syncs. The hub side, `record_sync`, already exists.

## Tasks

### T1: Investigate

Run, read-only:

```
kubectl get jobs -A | grep -i -E 'tenant|config|sync'
helm list -A | grep -i tenant-data
kubectl get applications -A | grep -i tenant
```

Write `docs/sync-job.md`, covering:

- the trigger;
- the image and script;
- how the Job gets the repos, and at which commits;
- the rename rule for each file type (Q3);
- the copy order;
- whether it deletes files;
- the NFS root;
- whether it's part of the `csp-tenant-data-<branch>` Helm chart (Q19).

Update `docs/open-questions.md`.

### T2: Recommend

Compare the options on four criteria: security, latency, changes needed to the Job, and how reliably each provides commit information.

- **(a)** The Job writes `.lanekeeper/sync.json` on NFS, and the agent reports it.
- **(b)** The Job calls a hub endpoint with a scoped token.
- **(c)** Nothing changes in the Job. The agent infers syncs from Job completion and Helm labels.

Recommend one.

## Acceptance

- [ ] `docs/sync-job.md` answers every point in T1.
- [ ] A recommendation with its trade-offs has been reviewed by a human.
