# Product brief

## Problem

The DevOps team releases a multi-microservice product with ArgoCD. Each swimlane (one GKE cluster, for example `sitb`) has its own NFS VM, and that VM holds the config files its config-server hands to the microservices.

Today the team reaches these VMs through IAP Desktop. That is slow for updates, and slower still for the common job of comparing files with the two GitHub repos (`configuration-base-saas` and `csp-tenant-data`) or with other swimlanes.

Changes start in Git or on NFS, depending on the person. A Kubernetes Job copies files from Git to NFS and replaces whole files without comparing them first. Nobody records which branch is deployed to which swimlane. Nobody records who changed what on NFS.

As a result, NFS edits can be silently overwritten, drift goes unnoticed, and an unreviewed change goes live at whatever moment the service next restarts.

## Users

- **DevOps engineers:** day-to-day edits, comparisons, PRs and restarts.
- **Admins:** user approval, roles, change approvals, swimlane registry and adoption.
- **Developers and testers** working in a swimlane: read access, docs search, and asking Cursor questions about configs.

## Goals

1. Any config on any swimlane can be compared with Git, its baseline or another swimlane in under a minute, without IAP.
2. Every change made through the tool has a named user. Every change made outside it is detected and flagged.
3. For every swimlane, the tool shows which tenant branch and base version it runs, and which files have drifted.
4. NFS changes reach Git through a PR raised from the tool, with the base-or-tenant consequence made explicit.
5. Users can ask Cursor what a config does, which swimlane has which value, which features are enabled where, and where inconsistencies are. Answers are grounded in computed facts and in the configuration docs.

## Non-goals for v1

- **The tool does not copy Git to NFS.** The sync Job stays the only copier.
- **No chat in the web UI.** AI lives in Cursor. The web UI does have a plain docs search box.
- **No per-swimlane permissions.** Roles are global.
- **No server-side LLM.** Cursor's model does the reasoning. A small embedding model for docs search is not an LLM.
- **Only `data/config` is in scope.** `data/atm-config`, `data/branches.json` and `data/tenants.json` come later.
- **These are v2:** GLiNER, the decision model, swimlane claims, tag creation, release snapshots, and the drift-from-template check.

## Quality priorities

- **Order of priority:** security, then performance, then efficient features. Implementation complexity is acceptable when it buys one of these.
- **Who builds it:** AI coding agents, with human review.
- **Where the details live:** the binding targets are in `docs/security.md` and `docs/performance.md`.

## Success metrics

Measure the "today" numbers before launch.

- **Time to compare one file across all swimlanes:** IAP baseline compared with the tool. Target under 1 minute.
- **Share of NFS changes with a named author:** target 95% or more within 2 months of launch.
- **Out-of-band changes detected per week:** should trend down as the tool replaces IAP.
- **NFS changes lost to sync overwrites (C7 findings):** target zero unnoticed. Every one alerted, and the lost version restorable.
- **Pilot adoption:** every DevOps engineer on the pilot swimlanes uses the tool weekly by milestone M3.
