# 12: Admin

## Prompt

```
Using the Lanekeeper design system, design the Admin area (Admin only), as a settings layout with a left sub-nav:

1. Users and access requests:
   - Pending requests (Sam Okafor), with "Approve as" and a role select, or Reject.
   - The users table: role, requires-approval toggle, status, last seen, linked GitHub login.
   - Safeguards shown: "You can't change your own role", "At least one active admin is required".
2. Swimlanes and agents:
   - Register a swimlane (name, project, cluster, tier, Workload Identity service account), or create a fallback join token (shown once, with expiry).
   - Agent list: version, replica, last heartbeat, certificate expiry.
3. Path mappings: rules per repo (repo prefix → NFS prefix, rename by branch suffix), with a live preview that maps a sample path both ways.
4. Merge modes: glob → whole_file or spring_merge, with a preview of the effective result for a sample file.
5. Host tiers: patterns (dev, uat, prod) with a test field ("video.dfbsaas-dev.example.net → dev").
6. Folder mappings: suggested NFS folder → Deployment, with a similarity score; confirm or correct.
7. Notifications: Teams webhook per notification type (masked after saving), with a test-send button.
8. MCP personal tokens (only when that mode is active).
```

## Follow-ups

- "Every rule editor needs a live preview so admins see the effect before saving."

## Done when

- [ ] Every admin page has its empty, editing, validation-error and saved states.
- [ ] Secrets (join tokens, webhook URLs) are shown once and masked afterwards.

## Additional prompt (sentinels, mappings, severity, retention)

```
Add admin pages:
- Sentinels: VM, swimlane, version, last heartbeat, last record, status. "Heartbeat gap" appears as a high-severity alert.
- OS Login user mapping: OS Login username → app user, auto-matched by email, with manual overrides.
- Severity rules: an ordered list (path glob, change type, finding type → severity), with a live preview against recent changes.
- Retention: versions kept (180 days, last 50 per file), with a dry-run "would free 3.2 GB, 0 referenced blobs affected".
```
