# Threat model (STRIDE)

Each prompt fills in the section for the components it owns, and keeps it current. A human signs off before go-live (S24).

For each component, record:

- **Assets.**
- **Trust boundaries.**
- **STRIDE threats:** Spoofing, Tampering, Repudiation, Information disclosure, Denial of service and Elevation of privilege. Give each threat its mitigation, with the S# it maps to.
- **Residual risks.**

## Components

- Agent (02)
- Hub identity and audit (03a)
- Hub platform: gateway, routing, events, Git, webhooks (03b)
- Engine (04)
- Registry and read API (05)
- Write paths and token vault (06)
- MCP server (07)
- Web app (08)
- Deployment and supply chain (09)
- Docs search (14)
- NFS VM sentinel (17)

## Known residual risks (from design)

- **Partly attributed NFS changes.** With the sentinel (D72), edits made on the VM are attributed to a named OS Login user. Writes from NFS clients that happen outside a sync Job are attributed only to "an NFS client", at medium confidence. A root user on the VM could stop auditd; the sentinel's heartbeat gaps are flagged as findings.
- **Cursor's model providers.** Config content sent through Cursor reaches Cursor's model providers. This is accepted because the files contain no secrets (D18), and the six flagged files are pending review (Q20).
- **Delayed lockout.** A user disabled in Entra keeps any active session until it is ended in the app or expires, which takes at most 12 hours. Mitigation: admins can disable the user in the app, which ends their sessions immediately.
