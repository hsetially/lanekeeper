# Review of prompt 08 (web app), round 1

Reviewer: independent `reviewer` agent, fresh context, read-only. Reviewed branch `agent/08-frontend` at `9c9243a` (commits `08/T0` to `08/T6`) against `origin/main` per `prompts/REVIEW.md`, on 2026-10-24. The text below is the reviewer's report, saved unchanged.

VERDICT: CHANGES REQUESTED

## Blocking

1. `web/pnpm-workspace.yaml:10`, `web/pnpm-lock.yaml`: the supply-chain gate is red and the recorded count is stale. `pnpm audit` on 2026-10-24 reports 17 findings (1 critical, 5 high, 8 moderate, 3 low), not 5.
   - dompurify 3.4.16 via monaco-editor: 1 critical, 1 high, 1 low. Fix is 3.4.17 (2026-10-12).
   - zod 4.x (via eslint-plugin-react-hooks and orval): 1 high. Fix is 4.6.6 (2026-10-11).
   - undici 7.x (via jsdom and orval): 3. Fix is 7.30.1 (2026-10-15).
   - fast-uri 3.x (via ajv): 4. Fix is 3.1.9 (2026-10-19).
   - @vitest/mocker: 2. Fix is vitest 4.1.12 (2026-10-15).
   - Every fix is under 14 days old, so `minimumReleaseAge` forbids it. They become eligible between 2026-10-25 and 2026-11-02.
   - The built editor chunk `dist/assets/entry-*.js` ships Monaco's vendored DOMPurify 3.4.15, which the `overrides` pin cannot reach. The "never runs" claim is backed by e2e/editor/csp.spec.ts.
   - Violates S19 and AGENTS rule 15 (osv-scanner must pass). Fix: a human records an exception in docs/decisions.md, or the bumps and overrides land after the age window. Do not merge before one of the two.
2. `.github/workflows/web-e2e.yml:10`: CI never enforces the P10 timing budgets.
   - The header says "the P10 performance gates and the filtered `cargo xtask bench-check` (08/T2)" are added later. They never were.
   - The workflow has only `e2e` (`just web-e2e`) and `visual-baselines`. `ci.yml` has no perf or bench step either.
   - The plan's Extra-paths row for this file promises `just web-perf` plus the filtered bench-check "so the thresholds are enforced in CI".
   - Scenario: route change p95 goes to 150 ms, or the grid starts dropping frames, and every CI job stays green. Only the bundle gate runs, inside `pnpm build`.
   - Violates P10 and AGENTS rule 14. Fix: add a perf job running `just web-perf` and `just web-bench-check`, or have a human record which other job does.
3. `web/src/lib/csp/trusted-types.ts:67` plus `web/e2e/csp/`: the Playwright XSS corpus for `SanitisedDocHtml` does not exist.
   - Plan "How the CSP test gets the production policy" and the S12 traceability row promise it: zero violations, zero script execution under the real header.
   - The repo has only a types-only unit test (`SanitisedDocHtml.test.tsx`) and one benign journey (`e2e/journeys/search.spec.ts:52-66`).
   - The `lanekeeper-docs` policy is a pass-through (`createHTML: (input) => input`), so the CSP and Trusted Types layers are the only guard if the server sanitiser fails.
   - Checked by hand in a scratch copy with a hostile `RenderedDoc` (script, `img onerror`, `javascript:` link, iframe, form, style attribute, stylesheet, `style` element).
     - Nothing executed: `window.__xss` stayed null.
     - The `javascript:` click did nothing. The iframe, form action, inline style and off-origin stylesheet were blocked (`frame-src`, `form-action`, `style-src-*`).
     - So the layers hold, but nothing in the repo proves it.
   - Fix: add `e2e/csp/xss.spec.ts` with a hostile doc scenario. Assert nothing executed, no off-origin request, and the expected violations.
4. `web/src/features/adoption/AdoptPage.tsx:7-8`: the adoption wizard (T4 S6b) is a `StubPage` saying "This screen is built in a later task".
   - Missing proofs: `features/adoption/Wizard.test.tsx` (traceability row "T4 component tests") and journey 6.
   - The Admin "Start adoption" action in the locked dialog, and the Adopt button in the swimlane list, lead to this stub.
   - It is blocked on CG6, which is not in contract PR #20. The prompt's acceptance is "all required".
   - Fix: land `contract/08-adoption` and build it, or have the human explicitly accept the deferral. The PR checklist must show it unticked.
5. `web/e2e/perf/grid-5000.spec.ts:1-60`, `perf/budgets.toml:612-621`, `docs/threat-model.md:960`: three requirement relaxations rest only on agent-written notes.
   - The notes are in `plans/08-frontend.blockers.md` (lines 344-352, 444-446, 516-518, 331) and in the threat model, relayed "by the orchestrator". There is no human-authored commit and no docs/decisions.md entry.
   - Under lanekeeper-security only a human can relax a requirement, recorded in docs/decisions.md.
   - W20: the P10 grid budget ("5,000-row grid holds 60 fps", described as "scroll rate") is now judged on uncapped capacity (`--disable-frame-rate-limit`).
     - The recorded value is 296.7 fps, and the vsync-locked 60 fps figure is informational only.
     - The plan said the threshold is never loosened and a miss is escalated.
   - S14b: "As served" shows `ServedFile.text` in clear to any Viewer. Decision D128 says the real hub also sends `FileContent`, `DiffLine` and `TextHit` text unmasked. The threat model's first residual risk names only the served text.
   - One-time tokens get a copy button (decision 11, `PersonalTokens.test.tsx:147` cites it).
   - There is also an optimistic-update lint exception and three prototype patches for Monaco (`editor/global-patches.ts`). Both contradict the frozen plan's lint list.
   - Fix: the human records these as D-entries (docs/decisions.md is not 08-owned) before merge. The PR text must list each one.
6. Gates the reviewer could not run: `just verify-08`, `just verify`, `cargo deny`, `cargo audit`, osv-scanner, and the 593 app-owned baselines (NOT RUN by design). The skill treats an unverifiable gate as blocking. CI must be green on them. The pieces that could be run are in Evidence.

## Non-blocking

1. Copy-paste that should be shared:
   - `isDocPath` is duplicated with different bounds, in `features/docs/doc-path.ts:14` and `features/file/docs-panel/model.ts:14`. It is a path validator, so the copies can drift.
   - `components/notify/OutcomeDialog.tsx` and `components/app/ProblemDialog.tsx:36` are near-twin dialogs, with the same `focusDialog`.
   - `saveDataOn` is in `shell/AppShell.tsx:41` and `routes/prefetch.ts:49`. `scheduleIdle` is in `editor/warm.ts:84` and `routes/prefetch.ts:38`.
   - `useFocusOnOpen` is in `audit/EventDrawer.tsx:66` and `approvals/ProposalDrawer.tsx:48`. The drawers repeat `Rows`, `K`, `V` and `Section`.
   - The redaction-marker regex is written 3 times: `components/masking/marker.ts:26`, `components/app/copy-text.ts:35` and `features/compare/model.ts:135`.
   - `OfflineBanner` has 3 copies (`findings/`, `compare/lanes/LanesScreen.tsx`, `compare/grid/GridScreen.tsx`).
   - The control-character check has 4 versions, and `formatBytes` and `initials` have 2 each.
   - `features/{file,prs,notify,served,compare}/test-utils.tsx` each re-define `stubApi` and `routes` next to `src/test/app.tsx`.
2. Production exports that nothing imports but a test covers:
   - `features/file/draft/model.ts:45` `pickupVariant` (a second, dead copy of `features/swimlanes/pickup.ts:8`)
   - `auth/SignOutButton.tsx:5`
   - `features/swimlanes/queries.ts:29` `swimlanePickupQuery`
   - `features/notify/paths.ts:50` `notificationCounts`
   - `features/compare/model.ts:212` `differingCells`
   - `features/search/results.ts:11` `hitSegments`
   - `components/app/RoleBadge.tsx:7`
   - `components/app/chips.tsx:79` `VersionChip`
3. Lint loopholes, probed with the real config in a scratch copy. None of these is flagged:
   - `window.eval(x)` (the base `no-eval` is not given the `window` global)
   - `el.setHTMLUnsafe(x)`
   - `Object.assign(el, { innerHTML })`
   - `const f = fetch; f(..)` and `globalThis["fetch"](..)`
   - `window["localStorage"]`
   - `createElementNS(.., "script")`
   - `window.postMessage`

   `lint/rules.test.mjs` proves only the listed snippets. Trusted Types and the CSP are the real guard; the threat-model line "no sink that takes a string" overstates the lint.
4. Secret-bearing operations reach `useSecretMutation` by convention (`PersonalTokens`, `TokenSetup`, `ChannelDialog`, `NotifySection`, `JoinTokenCard`, `use-reveal`). No lint rule or source scan stops a new caller of `createMcpToken`, `putGithubToken` and the others from using plain `useMutation`.
5. CI sequencing:
   - Missing app-owned baselines fail in CI (`e2e/visual/baseline-rule.mjs`), so `web-e2e` is red on this PR and on main until baselines are committed.
   - The `visual-baselines` job is `workflow_dispatch` only, and GitHub needs that file on the default branch before it can be dispatched.
   - So the bootstrap cannot run before merge. Sequence it in the PR, or let the e2e job skip baselines until they exist.
6. Intermittent failures:
   - `web/scripts/perf/lighthouse.mjs:57`: the first run died with `ENOTEMPTY` while removing the Chromium profile after SIGKILL, and nothing was written. A rerun passed. `rmSync` needs `maxRetries`.
   - `web/e2e/a11y/cases-file.ts:25-27`: types into Monaco with no wait for the editor to be ready. It failed once under concurrent load and passed 16 of 16 alone.
7. Acceptance 4 cannot be ticked:
   - 179 of 179 `design/DEVIATIONS.md` rows are `pending`.
   - Tier-1 uses wide masks (`CHROME_ONLY`, `LAYOUT`). File View 1m passes at 0.495% against the 0.5% limit with its whole page region masked.
8. Several traceability rows name files that do not exist (the plan is frozen, so the PR should map them). The behaviours are covered by the new names:
   - `inventory.test.tsx`
   - `OnboardingPage.test.tsx`
   - `client.test.ts` for CSRF
   - `useEditSession` and `SaveDialog` tests
   - `e2e/journeys/file.spec.ts:66` for CRLF
   - `features/masking/masking.dom.test.tsx`
   - `e2e/editor/csp.spec.ts` for the Monaco CSP
   - The "no AI chat" row also promised a chat-string ban in `scripts/dist-check.mjs`. It is absent; `routes/inventory.test.tsx` and `e2e/csp/requests.spec.ts` carry it.
9. Dependencies:
   - `@tailwindcss/node` is used only by `src/styles/tailwind.theme.test.ts`.
   - `vitest` 4.1.11 and `lighthouse` 13.5.0 are not in the plan's dependency table; each needs a PR justification.
   - All 43 direct dependencies are at least 14 days old (youngest is vite 6.4.4, 17 days).
10. Non-task commits: `08/blocked:` twice, `08/decision:` once, and two merges of origin/main. They touch only `plans/08-frontend.blockers.md`. Task order T0 to T6 is correct. No commit touches the plan after approval. The `approved by a human` commit carries the same author as the agent commits, as the standard flow does.

## Evidence re-run

- Scope: only owned paths plus the plan's Extra-paths changed (`web/**`, `design/DEVIATIONS.md`, `docs/threat-model.md`, `crates/hub/examples/web_fixture.rs`, `web-e2e.yml`, `budgets_registry.rs`, `perf/budgets.toml`, Justfile, blockers file). The contract diff is only the five `registered` flips, with no threshold change. All contract paths are otherwise unchanged against origin/main.
- `pnpm install --frozen-lockfile`: up to date.
- `pnpm lint`: exit 0.
- `pnpm typecheck` (`tsc -b`): exit 0.
- `pnpm exec vitest run`: 375 files, 9,309 tests passed.
- `pnpm gen:api`: no diff under `src/api` (fresh).
- `pnpm build`: ok. Initial JS 195.4 KB gz; dist-check clean over 154 files.
- `pnpm build:perf`: ok. 196.1 KB; dist-check clean over 168 files.
- `pnpm audit`: 17 vulnerabilities (blocking 1).
- Playwright `chromium`, full project, with `LK_REACT18_DIR` (tarballs matched the pinned sha512): 2,827 passed, 1 failed, 593 skipped.
  - The failure was a11y "axe, dark, file edit / the editor, a change made", under load from the reviewer's own parallel jobs.
  - The 593 skips are the app-owned baselines (NOT RUN).
- Playwright `perf` project, one spec at a time, on a quiet machine (load below 0.8):
  - route change p95 64.8 ms (budget 100)
  - diff of 6,000 lines p95 115.4 ms (budget 500)
  - grid capacity min 296.7 fps (floor 60); the vsync-locked figure was 60, with 0 late frames against the control list's 1
  - Lighthouse lowest page 93 (floor 90), on the second attempt (first aborted, see non-blocking 6)
- `cargo xtask bench-check` on the five P10 ids, after clearing stale results: 5 pass, 0 fail, 0 unmet (195.432 kb, 64.8, 115.4, 296.7, 93).
- `cargo fmt --all --check`: clean.
- `cargo clippy --locked -p hub --example web_fixture -D warnings`: clean.
- `cargo test -p xtask --test budgets_registry`: 8 passed.
- `scripts/check-actions-pinned.sh`: all pinned. The `upload-artifact` SHA matches `ci.yml`.
- Mutation checks in a scratch copy (each made the targeted tests fail): masks failing open; CSRF header removed; secret mutation not forgotten; `Last-Event-ID` dropped; reveal auto-hide removed.
- NOT RUN: `just verify-08`, `just verify`, `cargo deny`, `cargo audit`, osv-scanner, Docker, and the 593 app-owned baselines.
- Security trace:
  - S2: `auth/session.tsx` and `auth/signout.ts`, tested by `auth/session.test.tsx` and `lib/query/no-persist.test.ts`.
  - S3: `api/client.ts`, tested by `api/client.test.ts` (every write operation in the contract) and `api/constants.drift.test.ts`.
  - S12: `lib/csp/trusted-types.ts`, `editor/policy.ts`, `lint/lanekeeper-rules.mjs` and `scripts/dist-check.mjs`, tested by `e2e/csp/*.spec.ts` and `e2e/editor/csp.spec.ts`. The XSS-corpus proof is missing (blocking 3).
  - S14b: `components/masking/plan.ts` (fails closed), tested by `plan.test.ts` and `features/masking/masking.dom.test.tsx`.
  - S21: `components/app/ErrorState.tsx` and `api/problem.ts`, tested by `components/app/states.test.tsx`.
  - S19: red (blocking 1).
  - Threat model updated: yes, "Web app (08)" in `docs/threat-model.md`.
