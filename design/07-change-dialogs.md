# 07: Change dialogs (consequences before actions)

## Prompt

```
Using the Lanekeeper design system and its "consequence confirmation" dialog, design these flows:

1. RAISE PR from sitb / tx-infinity-core-sit7.yml
   Step 1, choose the target, with two large option cards:
   - "Tenant branch sit7 (this branch only)" → writes data/config/tx-infinity-api/tx-infinity-core.yml on branch sit7. Shows "Affects swimlanes running sit7: sitb".
   - "Base (all swimlanes)" → writes config/tx-infinity-api/tx-infinity-core.yml on the default branch. Warning: "This copies one tenant's content into base and affects every swimlane on its next sync."
   Show the case where the tenant file is new on that branch, with a fork warning: "Future base changes won't reach sit7 for this file."
   Step 2: title, description (prefilled with swimlane, hashes, audit id), "Create PR". Success state links to the GitHub PR.
   Error states: GitHub token expired (link to My token); no push access.

2. DELETE tenant file tx-infinity-core-sit7.yml
   Consequence: "After the next restart, tx-infinity-core in sitb will use tx-infinity-core.yml," with a semantic diff of what changes. Destructive confirm, which requires typing the file name.

3. UPLOAD a new tenant file into ui/remote-itm-teller
   Consequence variants:
   - "This forks ui-common-config.yaml from base";
   - "Suffix sit3 doesn't match the deployed branch sit7; this file will never be read in sitb".

4. RESTART
   - Restart tx-infinity-core in sitb: affected pods, and a warning "Other people testing in sitb will be interrupted".
   - Root-file variant: "application.properties affects all 14 services", with a checklist of services and "Restart all".

5. CONFLICT (409) while saving: "This file changed on NFS since you opened it." A three-pane view (your version, current NFS version, and the base you started from) with "Re-apply my changes on top" and "Discard mine".

6. SENT FOR APPROVAL (202, Ana Ruiz requires approval): a confirmation with a proposal link and its expiry (72 hours). Also the stale-proposal state, as seen later.

7. EDITING LOCKED (423): explains that adoption is needed, with a link for Admins.
```

## Follow-ups

- "Make every consequence readable in 5 seconds: lead with the effect sentence, then the details."
- "Show keyboard flow: Tab order, Enter to confirm only when safe, Escape to cancel."

## Done when

- [ ] Every flow shows its consequence before the confirm button.
- [ ] The success, error, conflict, approval and locked outcomes are all designed.
- [ ] Destructive actions need an explicit confirmation.

## Additional prompt (PR links and PR jobs)

```
Update the Raise PR flow:
- If an open PR already exists for this exact change: show "PR #418 is already open for this change" with a link, and no create button.
- After submitting: a PR job progress view (preparing → committing → opening → done) updating live, and resumable after a page reload.
- Drift list badges: "PR #418 open" and "Fix merged, awaiting sync".
```
