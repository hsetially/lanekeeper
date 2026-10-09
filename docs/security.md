# Security baseline

Every requirement here is mandatory. Prompts cite these IDs, and the reviewer agent checks them. A requirement can only be relaxed by a human decision recorded in decisions.md.

## Identity and access

- **S1 User identity.**
  - Microsoft Entra ID, single tenant, using OIDC with PKCE.
  - Validate the issuer, audience, `tid`, expiry and signature against cached JWKS.
  - Identify users by `tid` plus `oid`.
  - Roles live in the app database. Pending and disabled users get 403 everywhere, including MCP.
- **S2 Sessions.**
  - Stored server-side, with the cookie `__Host-lk_session`, set HttpOnly, Secure and SameSite=Lax.
  - Rotated at sign-in.
  - Timeouts of 8 hours idle and 12 hours absolute.
  - Ended immediately when a user's role or status changes.
- **S3 CSRF.** A double-submit token plus an `Origin` header check on every state-changing request.
- **S4 Default-deny authorization.**
  - Every route and MCP tool declares its required role through an extractor.
  - A test lists the whole route table and fails if any route lacks a guard.
  - The last active admin can't be removed, and admins can't change their own role.
- **S5 Agent identity.**
  - **Join:** the agent presents a Google-signed ID token for its Workload Identity service account, with audience = the hub. The hub checks the token's `email` against the service account registered for that swimlane.
  - **Fallback:** a one-time join token, stored hashed, valid for 24 hours, bound to one swimlane. Only for clusters without Workload Identity.
  - **Certificates:** client certificates are valid for 24 hours, with the SAN `spiffe://lanekeeper/swimlane/<id>`, and are renewed at half their lifetime.
  - **CA key:** it lives in Cloud KMS and is never exported. Certificates are signed through KMS.
- **S6 Transport.**
  - TLS 1.3 only, with rustls and aws-lc-rs.
  - HSTS on the web endpoint.
  - Mutual TLS between agents and the hub, and between hub replicas.
  - Postgres connections use TLS with full certificate verification.

## Secrets and data

- **S7 Secrets.**
  - Runtime secrets come from Secret Manager through Workload Identity, not from Kubernetes Secrets. The exception is the database credentials that CloudNativePG manages.
  - The hub authenticates to Entra with a federated credential where possible (Q25), otherwise with a client secret held in Secret Manager.
  - In code, secrets are `Secret<T>`, zeroized on drop, and compared in constant time.
- **S8 GitHub token vault.**
  - Fine-grained tokens only.
  - Envelope encryption: a fresh AES-256-GCM data key per token, with the user id and credential id as associated data, wrapped by a Cloud KMS key.
  - Decrypted only inside the owning user's request, and zeroized afterwards.
  - Never logged, returned in a response or sent to AI.
- **S9 Tamper-evident audit.**
  - Append-only (a trigger plus INSERT/SELECT-only grants), and hash-chained.
  - Every hour, the chain head is signed with a KMS key and written to a GCS bucket with a locked retention policy.
  - A nightly job verifies the chain against those checkpoints.
  - Events are also copied to Cloud Logging.
- **S10 Data handling.**
  - Logs contain paths, hashes and ids, never file contents or secrets.
  - GCS buckets and persistent disks use CMEK (Q29).
  - Backups are encrypted.

## Input and output

- **S11 Input validation.**
  - Typed parsers for `NfsPath`, `CompareRef` and path mappings.
  - Request bodies are capped at 4 MiB, and JSON nesting depth is limited.
  - Query parameters are bounded, and pagination is mandatory.
  - User-supplied regexes are compiled with the Rust `regex` engine, which runs in linear time, with size and DFA limits.
  - Uploads must be UTF-8 Markdown, at most 2 MiB.
- **S12 Output safety.**
  - Markdown is rendered on the server with pulldown-cmark, then sanitised with ammonia.
  - The frontend never injects raw HTML other than that sanitised output.
  - Strict CSP with nonces, Trusted Types, `frame-ancestors 'none'` and COOP `same-origin`.
  - No third-party origins. Monaco and fonts are self-hosted.
- **S13 Secret detection.** A value-based secret check blocks saves. The adoption pass scans both repos.
- **S14 MCP.**
  - Entra access tokens, checked for audience, scope `mcp.access`, `tid`, and that the user is active.
  - A role check on every tool.
  - Server-enforced elicitation before any write.
  - Output is labelled as data, and capped.
  - Per-user rate limits.
  - No tool executes a shell. The docs filesystem is virtual.
- **S14b Redaction.**
  - MCP output replaces values flagged by the value-based secret scan with `[REDACTED: N chars, rule]`.
  - The web UI masks flagged values by default. Revealing one is an audited action for Editors and above (D79).
- **S15 Prompt injection.** Config and doc text is always treated as data. The skill and MCP outputs say so explicitly. No server-side component sends user content to an LLM.

## Workload hardening

- **S16 Containers.**
  - Distroless or scratch base images.
  - A non-root uid, a read-only root filesystem, the RuntimeDefault seccomp profile, all capabilities dropped, and no privilege escalation.
  - Resource requests and limits set.
  - No service-account token automount unless the workload needs it.
- **S17 Agent.**
  - No shell or exec.
  - All file access goes through a cap-std `Dir` rooted at the NFS mount.
  - Minimal RBAC, with no access to Secrets apart from its own certificate Secret, by name.
  - Egress limited to the hub endpoint, where the cluster's NetworkPolicy support allows it.
  - Deny globs for keystores, keys and private material. Such files are hashed only, never sent and never written (D79).
- **S18 Network.**
  - NetworkPolicies deny by default.
  - The hub's egress is limited to Entra, GitHub, Google APIs, the Teams webhook host and Postgres.
  - Both load balancers have source-IP allowlists.
  - The GitHub webhook path accepts only GitHub's published hook ranges and verifies the HMAC signature.

- **S25 Sentinel (NFS VM).**
  - **Binary and packaging:** a static musl binary, shipped as a signed RPM and installed only through configuration management.
  - **systemd hardening:** a dedicated system user; `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`; an empty capability bounding set; `MemoryDenyWriteExecute`; `RestrictAddressFamilies` limited to AF_UNIX and AF_INET/AF_INET6.
  - **Reading audit events:** only from the audisp af_unix socket, through a group with read permission on it. The sentinel never reads `/var/log/audit` directly.
  - **What it handles:** no file content, ever. It sends only paths, operations, timestamps, OS Login usernames and executable names.
  - **Network:** no listening ports, and outbound mutual TLS to the hub only.
  - **Identity:** proven with the GCE VM's service-account ID token (S5).

## Supply chain

- **S19 Dependencies.**
  - Lockfiles are committed, and builds use `--locked`.
  - `cargo deny` (advisories, licences, bans, sources), `cargo audit` and `osv-scanner` all run in CI.
  - Renovate, with a minimum release age of 14 days.
  - GitHub Actions pinned by commit SHA.
  - Base images pinned by digest.
- **S20 Artifacts.**
  - A CycloneDX SBOM per image.
  - Keyless cosign signatures and SLSA build provenance attestations.
  - GKE Binary Authorization admits only signed images (Q24).
  - Image scanning in CI blocks critical vulnerabilities.

## Verification

- **S21 Code.**
  - `#![forbid(unsafe_code)]` in every crate we own.
  - Clippy denies panicking constructs outside tests.
  - Errors don't leak internals.
- **S22 Fuzzing.**
  - cargo-fuzz targets for every parser at a trust boundary: paths, compare refs, path mappings, YAML, JSON and `.properties` handling, Markdown chunking, webhook payloads, and proto decoding at the hub.
  - Smoke runs on every PR, and 10 minutes per target nightly.
- **S23 Dynamic testing.** An OWASP ZAP baseline scan against staging for every release, with no high-severity findings.
- **S24 Threat model.**
  - `docs/threat-model.md` covers every component using STRIDE.
  - Every prompt updates the section for its component.
  - A human signs off before go-live.
