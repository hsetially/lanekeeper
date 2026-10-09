# 00: Project brief (paste first)

Paste this block into the new Claude Design project. It gives Claude the context that every later prompt relies on.

```
You are designing Lanekeeper, an internal web app for a DevOps team. Don't design anything yet; read this brief and confirm you understand it in 5 bullet points.

WHAT IT DOES
The team releases a banking product made of many microservices. Each "swimlane" is one Kubernetes test cluster (for example sitb or presita), and each has an NFS server holding config files (YAML, JSON, .properties, XSL templates, images). Those files come from two Git repos:
- configuration-base-saas (base config, one branch, tags like v2.3.1);
- csp-tenant-data (tenant overrides, branches sit1 to sit13 and growing).
On NFS, a tenant file sits beside its base file with the branch as a suffix: tx-infinity-core.yml (base) and tx-infinity-core-sit1.yml (tenant). A service uses the tenant file if it exists, otherwise the base file.
Lanekeeper lets the team see which branch and base version each swimlane runs, compare files across swimlanes and with Git, detect drift, edit files safely, raise PRs, restart services, approve changes, search config and docs, and audit everything. AI help lives in Cursor, not in this app; there is NO chat UI.

USERS AND ROLES (cumulative)
- Viewer: browse, compare, search.
- Editor: edit files on NFS, upload, delete, raise PRs.
- Operator: also restart services.
- Admin: also approve changes, manage users and settings.
Some users are flagged "requires approval": their changes become proposals for an admin.
New users have no access until an admin approves them.

CORE CONCEPTS TO VISUALISE
- Drift states per file: In sync, Git ahead, NFS ahead, Conflict, Intentional, Unknown (no baseline yet), Untracked.
- Pending restart: a service is running old config until it restarts.
- Baseline confirmed or not: editing is locked until an admin completes "adoption" for that swimlane.
- Findings (C1 to C8): missed base changes, redundant tenant file, uneven tenant files, orphan file, drift, environment host mismatch, overwritten NFS change, duplicate keys.
- Live updates: data changes on screen without a refresh.

SAMPLE DATA (synthetic; use these everywhere)
- Swimlanes: sit, sitb, sitc (project ops-sit, tier SIT); presita, presitb, presitc (project ops-presit, tier PRESIT).
- Deployed branches: sitb runs sit7, sitc runs sit1, presita runs sit12. Base version "v2.3.1 + 4 commits".
- Files: tx-infinity-api/tx-infinity-core.yml and tx-infinity-core-sit1.yml; tx-infinity-api/entryGroupConfig.yml; ui/remote-itm-teller/ui-common-config.yaml; limits/limit-profiles.yml; security/security-roles.yml (6,100 lines); document-service/resources/receipt-logo.jpeg; ej-templates/deposit-receipt.xsl; application.properties (shared by all services).
- Services: tx-infinity-core, ui-service, limits-service, document-service, holds-service.
- Channels: remote-itm-teller, atm-iso, atm.
- People: Priya Shah (Admin), Marco Diaz (Editor), Lena Kowalski (Operator), Sam Okafor (pending access), Ana Ruiz (Editor, requires approval).
- Example setting paths: txInfinityOptions.remote-itm-teller.enableAccountSorting = true; entryGroupConfigMap.atm-iso.DEPOSIT.transactionKey.

DESIGN PRINCIPLES
1. Speed for experts: dense tables, keyboard-first, a Cmd+K command palette, a URL for every view.
2. Truth over decoration: every status shows a colour plus a text label plus an icon; never colour alone.
3. Consequences before actions: every change shows its effect (which swimlanes, which services, what changes) before confirming.
4. Calm under load: large files (6,000+ lines) and large grids (5,000 rows) stay smooth; design fixed-height rows for virtualisation.
5. Accessible: WCAG 2.2 AA, visible focus, full keyboard operation.
6. Built for shadcn/ui and Tailwind; self-hosted fonts; no external assets.
