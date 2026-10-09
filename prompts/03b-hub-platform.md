# 03b: Hub platform

| | |
|---|---|
| **Wave** | 1 |
| **Depends on** | 01 |
| **You own** | `crates/hub-platform/**`, `crates/hub/**` (binary composition), the platform sections of `docs/threat-model.md` |
| **Interfaces** | Implements `AgentGateway`, `BlobStore`, `GitReader`, `EventBus`, `Leases`, `KmsSigner`, `KmsEnvelope`, `SecretSource` and `Notifier` (no-op plus Teams later). Calls `ReportSink` (a fake until 05 lands). |
| **Security** | S5, S6, S7, S10, S11, S12, S16, S18, S21, S22 |
| **Performance** | P2, P7 (blob and SSE), P11, P13, P15 (acks) |
| **Gate** | `just verify-03b` |

## Objective

This is the runtime everything else sits on:

- agents join and connect to any of the 3 replicas;
- commands reach the replica that holds each agent's stream;
- events fan out to every replica and on to SSE subscribers;
- singleton jobs run exactly once;
- Git stays current through webhooks;
- blobs are served fast and can't go stale.

## Tasks

### T1: Composition and operations

`crates/hub` composes all the routers, applies the S12 headers, and serves `web/dist`:

- strict CSP with per-response nonces;
- Trusted Types;
- COOP `same-origin` and `frame-ancestors 'none'`;
- HSTS.

Operations:

- graceful shutdown, which drains SSE connections and agent streams;
- health and readiness endpoints;
- Prometheus RED metrics per route;
- tracing-opentelemetry exporting to Cloud Trace;
- every request passes through limits: a body size cap, a timeout, and per-user and per-IP rate limits.

**Verify:** a header test against every route class, and a test that oversized bodies are rejected.

### T2: Secrets and KMS (S5, S7, S8)

- **`SecretSource`:** the Secret Manager API, using Workload Identity, with values cached for at most 5 minutes.
- **`KmsSigner`:** asymmetric signing with Cloud KMS.
- **`KmsEnvelope`:** wrap and unwrap with Cloud KMS, using associated data.
- Each implementation has retries, timeouts and metrics.

**Verify:** conformance tests against the fakes. An opt-in test against a real KMS project exists, outside CI.

### T3: Agent CA, Join and Connect (S5, S6)

**CA.** rcgen builds each certificate, and `KmsSigner` signs it. The CA's private key never leaves KMS. Certificates are valid for 24 hours, with the SAN `spiffe://lanekeeper/swimlane/<id>`.

**`Join`:**

- With a Google ID token: verify it with `TokenVerifier::google_id_token`, and check that the email equals the service account registered for that swimlane.
- With a join token (fallback only): it's hashed, single-use, expires in 24 hours, and is compared in constant time.

**`Connect`:**

- Requires mutual TLS with TLS 1.3.
- The swimlane in the certificate SAN must equal the one in `Hello`.
- Proto decoding is size-limited.

**Verify:**

- join tests for every credential path;
- a test that forged, expired and wrong-swimlane certificates are rejected;
- a fuzz target for decoding `AgentMessage`.

### T4: Multi-replica routing (D62)

**Registration.** On connect, upsert `agent_connections` with (swimlane, replica id, internal address, epoch). On disconnect, clear it only if the epoch still matches.

**`AgentGateway::request`:**

- If the agent is connected to this replica, use the local stream.
- Otherwise, call the owning replica's internal `Forward` RPC over the headless service, with mutual TLS between replicas.
- Correlate by request id. Every request has a timeout and a bounded number of in-flight requests per agent.

**Reconnects.** When an agent reconnects to a different replica, the epoch increases, and in-flight requests to the old replica fail cleanly.

**Verify:** a test with three hub instances, where an agent moves between them during traffic. No request is lost without an error result.

### T5: Event bus and SSE (P13)

**`EventBus`:**

- In-process broadcast.
- Postgres `LISTEN/NOTIFY` for fan-out across replicas. Payloads are references only (ids and hashes, never content).
- Bounded subscribers. A lagging subscriber receives `Resync`.

**SSE at `/api/v1/events`:**

- Filters by what the user may see.
- Heartbeat every 15 seconds.
- Honours `Last-Event-ID` within a 5-minute replay window.

**Verify:** a test that delivery across three instances takes under 1 second (P13), and a test that a slow consumer gets `Resync` instead of blocking the bus.

### T6: Leases

`Leases` is implemented on Postgres, with time-to-live and fencing tokens. Singleton jobs (Git fetch scheduling, checkpoints, nightly runs) run under leases.

**Verify:** two instances race for a lease and exactly one holds it. When the holder dies, the lease is taken over after its expiry.

### T7: Git mirror and webhooks (P2, S18)

**Mirrors.** Each replica keeps a bare mirror of both repos, with all branches including `templates/*`, on its own persistent volume.

**Reading.** Through gix. The `GitReader` methods follow `docs/interfaces.md`. `tree_index` results are cached in moka and persisted to `git_tree_index` by the leaseholder.

**Webhooks at `POST /hooks/github`:**

- Verify the `X-Hub-Signature-256` HMAC in constant time.
- Deduplicate by delivery id.
- Accept push events only.
- Publish `GitHeadMoved`, and every replica fetches.

**Fallback.** The leaseholder polls every 60 seconds.

**Credentials.** A read-only service credential behind `GitCredentials` (Q17). User tokens are never used here.

**Verify:**

- webhook tests for a bad signature, a replayed delivery and a non-push event;
- a test that a push reaches `GitHeadMoved` within 10 seconds;
- a benchmark of `diff_trees` on fixture repos.

### T8: Blob store (P7)

- `BlobStore` on Postgres `blobs` (lz4 TOAST), with a moka cache weighted by size (default 512 MiB).
- `get_many` is batched.
- `GET /api/v1/blobs/{hash}` returns an immutable cache header and an ETag equal to the hash.

**Verify:** conformance tests, and a benchmark showing cached `get` under 1 ms and the blob endpoint under 30 ms (P7).

### T9: Threat model

Fill in the platform sections of `docs/threat-model.md`.

### T10: Transactional outbox (D80)

- `EventBus::publish_in_tx` inserts into `outbox` within the caller's transaction.
- A commit trigger issues `NOTIFY`. A relay on every replica reads the new outbox rows and fans them out.
- A leased job prunes rows older than 1 hour.
- Plain `publish` is only for events with no database change.

**Verify:** a rolled-back transaction emits no event, and a committed one is delivered exactly once per subscriber.

### T11: Sentinel endpoint, acks and PR webhooks

- **Sentinel service:** joins with the VM service-account ID token, which must match a registered sentinel for that VM. Connections use mutual TLS, and batches are passed to `SentinelSink`.
- **Agent acks:** after `ReportSink::delta` commits, send `Ack{seq}`.
- **Webhooks:** `POST /hooks/github` also accepts `pull_request` events, with the same HMAC check and replay protection, and publishes `PrStateChanged`.

**Verify:** tests for sentinel join with a wrong service account, ack ordering, and `pull_request` webhook handling.

## Acceptance (all required)

- [ ] `just verify-03b` passes, and P2 and P13 plus the blob-endpoint budget are within limits in `bench-check`.
- [ ] The three-instance tests for routing, events and leases pass.
- [ ] The CA key is only ever used through `KmsSigner`. A test fails if any code path loads a private CA key.
- [ ] Webhook verification and the fuzz targets pass.

## Stop and escalate if

- Internal gRPC between replicas isn't possible on the headless service in your cluster.
- Postgres `NOTIFY` can't keep up with P13 at design scale. If so, propose an alternative with measurements.

## Out of scope

Ingestion and drift (05), writes (06) and the identity crate (03a).
