# 17: NFS VM sentinel (attribution)

| | |
|---|---|
| **Wave** | 2. Can start against fakes after 01. |
| **Depends on** | 01. Integrates with 03b (sentinel endpoint) and 05 (`SentinelSink`). Installing on real VMs needs Q30. |
| **You own** | `crates/sentinel/**`, `deploy/sentinel/**` (RPM spec, systemd unit, audit rules, plugin config), the sentinel section of `docs/threat-model.md` |
| **Interfaces** | The `Sentinel` service in `proto/agent.proto` |
| **Security** | S5, S6, S10, S16, S21, S22, S25 |
| **Performance** | P14. Uses at most 20 mCPU and 32 MiB on the VM at steady state. |
| **Gate** | `just verify-17` |

## Objective

Tell the hub **who** changed a file when the edit was made directly on the NFS VM, for example through IAP Desktop.

Two facts make this work:

- OS Login is on, so a login identity maps to a real Google identity.
- auditd keeps that original login identity even after `sudo`.

Writes that arrive through NFS produce no local audit record, so the hub can tell them apart from local edits.

The sentinel never reads or sends file content. It reports metadata only.

## Tasks

### T1: Audit plumbing (`deploy/sentinel`)

- An auditd rule template: `-w <export_root> -p wa -k lanekeeper`. Generate one rule per configured root.
- The audisp af_unix plugin configuration (audit 3.x on Rocky Linux 8), writing to a socket that only the `lanekeeper-sentinel` group can read.
- Document how to verify locally: `ausearch -k lanekeeper` shows an edit made with `sudo vi`, with the original login identity of the OS Login user.

**Verify:** a Rocky Linux 8.10 container test that installs the rule, makes an edit as a test user through `sudo`, and confirms the event reaches the socket.

### T2: Parsing records

- Read the socket and assemble multi-record events (SYSCALL, PATH, CWD, PROCTITLE) by event id.
- Keep only events under configured roots.
- Extract:
  - time;
  - path (resolved from the working directory and relative paths);
  - operation (write, rename, unlink, create, attribute change);
  - success;
  - the login user and the effective user, resolved to usernames through NSS (OS Login usernames);
  - `exe` and `comm`.
- **Bounded buffers.** Malformed records are counted and skipped, never a cause of a crash.

**Verify:**

- golden tests from captured Rocky 8.9 and 8.10 audit samples (synthetic paths only);
- a fuzz target for the record parser;
- the login user survives `sudo` and `su`.

### T3: Identity and transport (S5, S6)

- Join with the GCE metadata ID token (audience = hub), and get a 24-hour mutual-TLS certificate, renewed at half its lifetime.
- Stream `AuditRecordBatch` messages (at most 500 records, or one per second) with sequence numbers. Acknowledged records leave a small local spool, capped at 50 MiB.
- No listening ports.

**Verify:** a fake hub test covering join, batching, acks, and replay after a disconnect.

### T4: Service hardening (S25)

- A static musl binary with `#![forbid(unsafe_code)]`.
- The systemd unit:
  - a dedicated user;
  - `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`;
  - an empty `CapabilityBoundingSet`;
  - `MemoryDenyWriteExecute=yes`;
  - `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`;
  - `ReadWritePaths` limited to the spool directory.
- Logs go to the journal and contain paths and usernames only.

**Verify:** `systemd-analyze security lanekeeper-sentinel` gives an exposure score of 2.0 or lower, and the service starts under every restriction.

### T5: End-to-end correlation

Run with prompt 05's fakes.

- An edit on the VM made with `sudo` produces a `sentinel_login` attribution, with high confidence and the right user, within 30 seconds (P14).
- A write through an NFS client mount produces no sentinel record, so the attribution falls back to `sync_job` or `nfs_client`.

### T6: Threat model

Fill in the sentinel section of `docs/threat-model.md`. Cover:

- compromise of the VM itself;
- a user with root who stops auditd. Gaps in the sentinel's heartbeat are surfaced as a high-severity finding.

## Acceptance (all required)

- [ ] `just verify-17` passes, and P14 and the resource budget are met.
- [ ] No file content is read or sent. A test asserts the binary never opens files under the export root.
- [ ] The signed RPM installs on Rocky Linux 8.9 and 8.10.
- [ ] A heartbeat gap or an auditd outage raises a high-severity finding in the hub.

## Stop and escalate if

- The audisp af_unix plugin isn't available in the installed audit version.
- OS Login usernames can't be resolved to identities (Q32).
- Q30 is refused. The fallback is to keep the agent-side attribution (`sync_job`, `nfs_client`) only.

## Out of scope

Detecting changes. The in-cluster agent's scans remain the source of truth.
