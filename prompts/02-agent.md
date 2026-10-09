# 02: Agent

| | |
|---|---|
| **Wave** | 1 |
| **Depends on** | 01 |
| **You own** | `crates/agent/**`, the agent sections of `docs/threat-model.md` |
| **Interfaces** | Speaks `proto/agent.proto`. Tested against a fake hub built from `crates/proto`. |
| **Security** | S5, S6, S10, S11, S16, S17 (including deny globs), S21, S22 |
| **Performance** | P1, P3, P4, P15 |
| **Gate** | `just verify-02` |

## Objective

The agent is a small, hardened binary, one per swimlane cluster. It mounts the config-server's NFS export and does five things:

- proves its identity using Workload Identity;
- keeps a Merkle tree of the config root;
- streams compact deltas to the hub;
- performs file writes that are byte-exact and checked against the expected hash;
- reports Deployments and pods.

It never decides anything. It reports facts and carries out commands.

## Tasks

### T1: Bootstrap

**Configuration** from environment variables:

- hub endpoint and hub audience;
- swimlane id;
- NFS mount root;
- the name of the certificate Secret;
- namespaces to watch;
- the name of the join-token fallback Secret.

**Startup:**

- Open the NFS root as a cap-std `Dir`. Every later file operation goes through this handle (S17).
- Fail fast, with a clear error, if the mount is missing or not readable.

**Verify:** unit tests for config parsing, and a startup test against a temp directory.

### T2: Join and certificates (S5)

1. Generate an ECDSA P-256 key in memory. Never write it to disk outside the certificate Secret.
2. Get a Google-signed ID token from the metadata server for the configured audience. Use the join token only if Workload Identity isn't available (Q26).
3. Call `Join` with the swimlane, the CSR and the credential.
4. Store the certificate and key in the certificate Secret, by name.
5. Renew the 24-hour certificate at 50% of its lifetime, over the stream.
6. If renewal fails, retry with backoff. When the certificate is less than 10% from expiry, fall back to re-joining.

**Verify:** tests with a fake metadata server and a fake hub cover token join, the token fallback, renewal, and recovery from an expired certificate.

### T3: Transport

- tonic with rustls, TLS 1.3 only, and a pinned hub CA.
- zstd compression on the stream.
- HTTP/2 keepalive.
- Reconnect with exponential backoff, full jitter and a 60-second cap.
- All outbound queues bounded. Delta messages are batched to at most 3 MiB.
- The transport sits behind a trait, so a WebSocket-over-HTTPS implementation can be added later.

**Verify:** a fake hub test that restarts the hub 20 times. The agent recovers each time, and its memory stays flat.

### T4: Merkle tree and scanning (P1, P3, P4)

**Walking and hashing:**

- Every 10 seconds, stat-walk the root with `ignore::WalkParallel`, using the ignore globs. Don't follow symlinks.
- Rehash only files whose size, mtime, ctime or inode changed.
- Every 15 minutes, fully rehash everything, because NFS attribute caching can hide changes.
- All hashing runs with rayon inside `spawn_blocking`.

**Merkle tree:**

- Keep the tree as defined in `docs/domain-model.md`.
- Send the root hash in a heartbeat every 10 seconds.
- On `RequestDelta(since_root)`, send changed entries with their raw bytes, plus removals and skips. Keep a ring buffer of recent roots, so a delta can be computed from any root seen in the last hour. Otherwise, send a full listing.

**Limits:**

- Files over 2 MiB are skipped and reported.
- Raw bytes are never decoded or normalised.
- Logs contain paths and hashes only.

**Verify:**

- property tests: the Merkle root is independent of walk order, and any single-file change changes the root;
- a criterion benchmark for a full rehash of 2,000 files (≤ 20 s, P4) and a stat walk (≤ 1 s);
- a scripted test where a file changes and the delta arrives at the fake hub within 15 seconds (P1).

### T5: File operations

**Path handling.** Every path goes through `NfsPath` and then cap-std. Reject symlink targets explicitly.

**Concurrency.** Serialise operations on the same path with a bounded per-path lock map.

**`WriteFile`:**

1. Read the current hash.
2. Compare it in constant time with the expected hash, which may be "must not exist". On mismatch, return a conflict with the current hash.
3. Write the exact bytes received to a temp file in the same directory.
4. fsync the temp file.
5. Copy the original file's mode onto it.
6. Rename it over the target.
7. fsync the directory.
8. Update the Merkle tree immediately.

**`DeleteFile`** uses the same precondition. **`ReadFile`** returns the bytes and their hash.

**Verify:**

- tests for traversal, absolute paths, symlinked files and directories, and NUL;
- a hash mismatch leaves the file untouched;
- CRLF content and a BOM round-trip byte-exact;
- a simulated crash before the rename leaves the original intact;
- binary files round-trip;
- a fuzz target for the path handling.

### T6: Kubernetes (S17)

- **Watching.** kube-rs watchers and reflectors, not polling, for Deployments, Pods and Jobs in the configured namespaces.
- **Reporting.** Send `ClusterReport` deltas on change, debounced to 1 second. Send a full report when asked.
- **Release hints** (Q19): read only the standard Helm labels (`app.kubernetes.io/instance`, `helm.sh/chart`), filtered by a configured pattern.
- **Env hints:** only env vars whose names are on the allowlist.
- **Restart:** merge-patch the `kubectl.kubernetes.io/restartedAt` annotation, only in configured namespaces.
- **RBAC.** Write `crates/agent/RBAC.md`:
  - get, list and watch on Deployments, Pods and Jobs;
  - patch on Deployments;
  - get and update the certificate Secret by `resourceNames`;
  - no Secret list permission.

**Verify:** tests with a fake API server cover watch-driven reports, the restart patch, refusal outside the configured namespaces, and an RBAC manifest lint that rejects Secret list.

### T7: Operations and hardening (S16)

- **Health and metrics:**
  - `/healthz`;
  - `/readyz`, which is ready only while connected;
  - Prometheus `/metrics`: scan durations, files tracked, delta sizes, connection state, and operation counts by result.
- **Container:**
  - distroless image, non-root uid and gid from values (Q11);
  - read-only root filesystem, apart from the NFS mount and a temp directory;
  - all capabilities dropped and the RuntimeDefault seccomp profile.

**Verify:** `just verify-02` runs everything above, plus the benchmarks, through `bench-check`.

### T8: Threat model

Fill in the Agent section of `docs/threat-model.md`.

### T9: Durable spool (D74, P15)

- Every observed version (path, hash, bytes, `observed_at`, `during_job`) is appended to a spool on a small persistent volume before it's sent.
- Entries are deleted when the hub acknowledges their `seq`.
- On reconnect, the agent replays in order. Entries are deduplicated by (path, hash).
- **Bounds:** 512 MiB or 100,000 entries, both configurable. On overflow, drop the oldest entries and report a `gap` record with the time range lost.
- Writes are crash-safe: append with fsync batching, and checksummed records.

**Verify:**

- kill the hub for an hour while files change A → B → C; after replay, all three versions arrive in order (P15);
- a corrupted tail record is skipped and reported;
- a crash test for the spool.

### T10: Quiescence and sync tagging (D75)

- Report a delta only after 3 seconds without further changes in the tree. If changes keep arriving, report after 30 seconds anyway.
- The Job watcher (T6) detects sync Jobs by a configurable label or name pattern. While one is active, every observed entry gets `during_job`, and a `SyncWindow` start event is sent. A `SyncWindow` end event follows when the Job completes.

**Verify:** a synthetic 600-file copy during a fake Job produces at most two deltas, every entry is tagged, and the window events arrive. P1 still holds when no Job is running.

### T11: Deny globs (D79)

- Default globs: `*.jks`, `*.p12`, `*.pfx`, `*.pem`, `*.key`, `*.keystore`, `*private*`. They're configurable through `AgentConfig`, but these defaults can't be removed.
- Matching files are hashed in a streaming fashion and reported with `denied=true` and no bytes.
- `ReadFile` and `WriteFile` refuse denied paths.

**Verify:** a test that no bytes from a denied file ever appear in any outbound message (capture the stream), and that reads and writes of denied paths are refused.

### T12: Config-server integration (D83, D85, D86, D88)

**Mounting:**

- Mount the same NFS path the config-server uses (`{nfsMount}/{cluster}/{namespace}/csp-configuration`), read-write, through the agent's own volume.
- Run as uid and gid 1010, which owns the files today.
- The config-server's own volume stays read-only.

**`NotifyConfigServer`:** POST to `http://<config-server-service>:8888/update-resources`.

- Form field `path`, repeated, with paths relative to the config root.
- Header `backend: filesystem`.
- Timeout 5 seconds, with bounded retries.
- Return the HTTP status code.

**`FetchServed`:** `GET /{application}/{tenant},default/master[/{channel}]/{file}`.

- Binary files use `Accept: application/octet-stream`.
- Return the bytes and the status code.
- At most 5 requests per second per agent, and responses are capped at 2 MiB.

**`ClusterReport`:**

- For each Deployment: values of the allowlisted variables, and the names only of all others.
- The config-server pod's start time.

**RBAC and network:** no new RBAC is needed. The agent's network policy must allow egress to the config-server Service on port 8888.

**Verify:**

- a fake config-server records notify posts with the right form, header and paths;
- `FetchServed` respects the rate limit;
- a log and stream capture shows no environment-variable values outside the allowlist.

## Acceptance (all required)

- [ ] `just verify-02` passes, and P1, P3 and P4 are within budget according to `bench-check`.
- [ ] Idle traffic is under 1 KB per minute, measured with the fake hub over 10 minutes (P3).
- [ ] Every file operation goes through cap-std, and the traversal and fuzz tests pass.
- [ ] The RBAC manifest grants no Secret list.

## Stop and escalate if

- The metadata server isn't reachable for ID tokens in a test cluster (Q26).
- NFS behaviour (attribute caching, rename atomicity) breaks the stated guarantees in a real mount test.

## Out of scope

Drift logic, Git, line-ending handling, and anything the hub computes.
