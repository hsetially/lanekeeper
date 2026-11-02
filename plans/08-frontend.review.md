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

# Round 2

Reviewer: independent `reviewer` agent, fresh context, read-only. Reviewed branch `agent/08-frontend` at `e082335` (follow-up commits `44d92da`, `7d69356`, `e082335` after `e5ed17e`) against `prompts/REVIEW.md` and the `lanekeeper-pr-review` skill, on 2026-11-02 (11:20Z to 12:30Z). Nothing was edited, staged or committed; build output went to ignored folders and the scratchpad.

VERDICT: CHANGES REQUESTED

## Blocking

1. `web/e2e/journeys/attribution.spec.ts:238-247`, `web/e2e/visual/pickup/runner.ts:116-125`, `web/e2e/editor/csp.spec.ts:80` (against `:92-102`): the gate of acceptance 1 is not reliably green on the committed tree. `just verify-08` runs `just web-e2e` with `retries: 0`. The follow-up characterised the two failures of main's last `web-e2e` run and left them unfixed, with a tested fix on file; I found a third flaky site.
   - (a) `attribution.spec.ts:238`: the route handler awaits `route.fetch()` and `response.json()` with no try/catch and outlives the test body, so the fixture teardown disposes the response. Proof: `playwright test --project=chromium --workers=3 --repeat-each=120 -g "the Drift tab says when its filter reaches only the rows that are loaded" e2e/journeys/attribution.spec.ts` gave `3 failed`, `117 passed (2.0m)`, each `Error: apiResponse.json: Response has been disposed` at `attribution.spec.ts:243`. The implementer's own counts are 1 of 30 and 5 of 262 (`plans/08-frontend.blockers.md:1354-1357`).
   - (b) The app-owned frame served S22 (light). `runner.ts` masks the live pill (the comment at `:121` says it "counts down to the next try with a jitter"), but a mask hides the pill and not the re-layout of its neighbours. Proof: `playwright test --project=chromium --workers=2 e2e/visual` gave `1 failed`, `743 passed (13.5m)`, with `served-S22-light.png: 6694 pixels (ratio 0.01 of all image pixels) are different`. In the diff image the switcher is at x 714-979 in the baseline and x 838-1103 in the capture, and the magenta mask is 180 px wide in the baseline and 56 px in the capture. The same frame alone, `--repeat-each=5`, passed 5 of 5.
   - (c) `csp.spec.ts:80` reads `fidelity(page)` in the task of the edit, while `:92-102` of the same file waits a bounded loop for the next animation frame and its comment records the same class of failure ("failed that way about one run in ten"). Main's CI message (`worst 19`, expected 27, received 0) is what that sample reads before the frame. Read, not reproduced: 60 of 60 passed under six busy loops here; the CI failure is relayed by the evidence (lines 124-129), not seen by me.
   - Violates acceptance 1 (evidence line 59 itself leaves the recipe unticked for this reason) and `REVIEW.md` section 4 (tests must not race).
   - Fix, using what the implementer already measured: end every test in `web/e2e/fixtures/test.ts` with `await page.unrouteAll({ behavior: "ignoreErrors" })` (150 of 150 on a scratch copy, `blockers.md:1382`; `behavior: "wait"` hangs) or wrap the handler body in try/catch; use the settle loop of `csp.spec.ts:94-102` at `:80` and make `:83` check the typed text; make the S22 frame deterministic (wait for the pill's text to be steady before the 250 ms pause at `runner.ts:117`, or pin the pill's box in frame mode) and look at every other frame that shows a refused stream. Then run each fixed test with `--repeat-each` of at least 150 and the whole visual folder once, and put the counts in the evidence.

## Non-blocking

1. `pnpm audit` is red: 7 findings (5 moderate, 2 low) on `fast-uri` 3.1.8 (4), `vite` 6.4.4 (1, dev server) and `minimatch` (1, two paths). Each fix is under 14 days old by the registry clock at 11:24Z: `fast-uri` 3.1.9 is 13.51 days old (eligible 2026-11-02T23:05:26Z), `vite` 6.4.5 is 12.24 (eligible 2026-11-04T05:35:48Z), `minimatch` 11.0.1 is 5.54 (eligible 2026-11-10T22:26:03Z). Round 1 had 17 findings with 1 critical and 5 high; `e5ed17e` had 29 with 10 high; now none is high or critical. `osv-scanner.toml:10-13` records that a human chose to wait, so the `osv` job of `web.yml` stays red until those dates. Merge precondition for a human: wait, or record an exception in `docs/decisions.md`. Not an agent defect.
2. D129 to D133 (`docs/decisions.md:382-412`: grid capacity gate, unmasked file text and As served in clear, copy button for one-time secrets, optimistic `adminPutAutoNotify`, Monaco prototype patches) are still "PROPOSED, not yet decided". The evidence's "Open human decisions that stand" list (lines 147-153) omits all five, and its Assumptions (line 138) say W20 was "decided by the human on 2026-10-17", which `docs/decisions.md` does not show. Only a human can relax a requirement. Fix: list the five with their status.
3. The adoption wizard is still a `StubPage` (`web/src/features/adoption/AdoptPage.tsx:7-8`) and `api/openapi.yaml` has no adoption operation (CG6 is not in the contract). The evidence leaves it unticked (line 65). It needs the human's explicit acceptance or `contract/08-adoption`; the prompt says acceptance is all required.
4. `design/DEVIATIONS.md`: 179 of 179 rows are `pending`. The follow-up edit (`7d69356`) changes one row, wording only (1 insertion, 1 deletion; the last cell is still `pending`). I checked its claims against `src/features/shell/crumbs.ts` (every folder is a crumb; an ellipsis above nine parts), `current-swimlane.ts` (the last swimlane is kept), `web/e2e/visual/file/masks.ts:20` (mask x 237, y 9, 619 by 25; 1,099 of 1,264 differing pixels) and the Shell reference screen (`showCrumb` only on file pages). They hold.
5. `@tailwindcss/node@4.3.3` (`web/package.json:37`) is a direct devDependency used only by `src/styles/tailwind.theme.test.ts:7`, and nothing in `plans/` or `docs/` justifies it (rule 15). `lighthouse` is justified at `blockers.md:429`; vitest is in the fixed stack.
6. The frozen plan's traceability table names about 30 proof files that no longer exist (for example `lint/rules.test.ts`, `api/client.csrf.test.ts`, `features/adoption/Wizard.test.tsx`, `routes/inventory.test.ts`, `e2e/journeys/j1.spec.ts`, `e2e/csp/monaco.spec.ts`, `e2e/editor/crlf.spec.ts`, `tsconfig.test.ts`). Real substitutes exist for the rows I sampled (`routes/inventory.test.tsx`, `routes/links_resolve.test.tsx`, `lib/csrf/csrf.test.ts`, `features/file/edit/SaveDialog.test.tsx` and `EditPane.test.tsx` for CRLF, `e2e/editor/csp.spec.ts`, `e2e/journeys/*.spec.ts`); the Wizard row has none. The evidence maps only the S# lines. A full name-to-name table belongs in the PR text; `plans/08-frontend.pr.md` does not exist.
7. The 684 app-owned baselines (`94bf740`, bot, Chromium 153) are untouched by the follow-up (0 paths in `git diff e5ed17e..HEAD`). The bot's commit message asks for a human look at the images; the evidence admits nobody did. Here, on Chromium 141, 743 of 744 visual tests passed (the failure is blocking 1b).
8. `docs/threat-model.md:846` still says `pnpm audit` found "no known vulnerabilities at the T1 commit". It is dated, so not false, but 7 findings and the scoped override `@scalar/json-magic>undici` (`web/pnpm-workspace.yaml:18`) are not mentioned. One sentence would do. Thin margins stay as the evidence says: Lighthouse lowest page 93 against 90 (mine 93, the file diff page); tier-1 File View 1c is at 0.478 percent of 0.5 in the runner's text rendering.
9. During my 60-repeat editor run the sandbox's egress proxy reported 136 refused connections to `www.google.com:443` and `android.clients.google.com:443`. `csp/requests.spec.ts` and `csp/csp.spec.ts` (same-origin requests only) passed in the same session and the shipped `dist` is byte-identical to the previous lockfile's build, so I attribute them to Chromium's own background services; I did not trace them.

## Round-1 findings

| ID | Closed | How |
|---|---|---|
| B1 audit, S19 | No (reduced) | Bump commit `44d92da` takes every fix that is 14 days old by the clock. `pnpm audit`: 29 (10 high) at `e5ed17e`, 7 (0 high) now, both run here; round 1 had 17 with 1 critical. The three left are 13.5, 12.2 and 5.5 days old; a human chose to wait (`osv-scanner.toml:10-13`). All 17 added and all 770 locked versions are at least 14.98 days old, integrities match. Non-blocking 1. |
| B2 P10 not enforced in CI | Yes | `.github/workflows/web-e2e.yml:174-213`: a `perf` job runs `just web-perf` and `just web-bench-check`, no `continue-on-error`, pinned by `web/scripts/perf/workflow.test.mjs`. I re-ran the five P10 budgets: 5 pass. There is no CI number for the job. |
| B3 XSS corpus | Yes | `web/e2e/csp/xss.spec.ts` (574 lines) and `xss-corpus.ts`; run here: 49 passed (3.4 min). The meta-refresh gap was closed in `features/docs/reader-html.ts`. |
| B4 adoption wizard | No | Still a stub, CG6 not in the contract; unticked in the evidence; the human's deferral is on record (`blockers.md:328`, `:1059`). Non-blocking 3. |
| B5 requirement relaxations | No | D129 to D133 are drafted (`69b7ea0`) but "PROPOSED, not yet decided"; the evidence does not list them as open. Non-blocking 2. |
| B6 gates not run | No | `just verify-08`, `just verify`, cargo deny, cargo audit, osv-scanner (api.osv.dev answers 403 here), Docker: not run. Every cheap step of `verify-08` was run as a command and passed (below). |
| NB1 copy-paste | Yes | `isDocPath` has one definition (`features/docs/doc-path.ts:14`); marker regex in `components/masking/marker.ts:26-27`; `focus-dialog.ts`, `lib/idle.ts`, `features/shell/focus.ts`, `lib/text/control.ts` exist; whole vitest passes. |
| NB2 dead exports | Yes | None of the eight names has an importer in src, e2e, scripts or lint; `differingCells` is a local const in `compare/model.test.ts:122`. |
| NB3 lint loopholes | Yes | Scratch probe with the real config flagged `dangerouslySetInnerHTML`, `window.eval` (twice), `setHTMLUnsafe` and a bare `fetch` (5 errors); `lint/rules.test.mjs` passes in the node project. |
| NB4 secret mutations by convention | Yes | `web/src/lib/query/secret-usage.test.ts` exists and passes. |
| NB5 CI sequencing | Yes | 684 baselines committed by the CI job and untouched since; visual folder 743 of 744 here. Caveat: non-blocking 7. |
| NB6 intermittent failures | Yes | `remove-dir.mjs` with its test; Lighthouse ran to the end here; `-g "file edit"` a11y 16 of 16. New flakes elsewhere are blocking 1. |
| NB7 acceptance 4 | No | 179 of 179 pending; tier-1 103 of 103 pass. Non-blocking 4. |
| NB8 traceability names | No (partial) | Substitutes exist for the rows sampled; no full map. Non-blocking 6. |
| NB9 dependencies | No (partial) | `@tailwindcss/node` unjustified. Non-blocking 5. |
| NB10 non-task commits | Yes | T0 to T6 in order (`cc3448c` to `9c9243a`); the three follow-up commits (`08/fix`, `08/fix`, `08/evidence`) touch only `web/`, `design/DEVIATIONS.md` and `plans/` and end with the two required lines; `git log 514c48a..HEAD -- plans/08-frontend.md` is empty; the blockers file is append-only (119 insertions, 0 deletions). |

## Specific checks asked for

- S19: 17 versions added and 18 removed in the lockfile since `e5ed17e` (`@puppeteer/browsers` 3.2.5, `@vitest/*` x7 4.1.12, `chromium-bidi` 157.0.8090-0, `devtools-protocol` 0.0.1696802, `modern-tar` 0.8.6, `puppeteer-core` 25.14.0, `seroval` 1.6.11, `undici` 7.30.1, `vitest` 4.1.12, `webdriver-bidi-protocol` 1.0.1, `zod` 4.6.7). Registry publish times at 11:24Z: the youngest is `seroval` 1.6.11 at 14.98 days; all 17 integrities equal the lockfile; none deprecated; no tarball or git source in the lockfile. A sweep of all 770 locked versions found none under 14 days. `minimumReleaseAge: 20160` is unchanged; the one new override (`@scalar/json-magic>undici: 7.30.1`) is explained in the file, and `@scalar/json-magic` 0.16.1 does depend on undici 7.30.1 as its comment says.
- S12 and CSP unchanged, no new egress or eval: no path under `crates/`, `web/src` or `web/e2e` changed. `pnpm build` gives `dist` byte-identical (`diff -rq`, 162 files) to a build of the `e5ed17e` tree with its own lockfile, same initial chunk `index-Dxfk2tYC.js`; `dist-check` scanned 158 files and found nothing the CSP would refuse; 305 CSP, editor and keymap tests and 49 XSS tests passed.
- The follow-up touched no contract path and not `docs/decisions.md` (0 of 6 changed files). The five P10 entries of `perf/budgets.toml` still carry thresholds 250, 100, 500, 60, 90 and `registered = true`.
- The 684 baseline PNGs are untouched by the follow-up. The DEVIATIONS edit is wording only and the row stays `pending`. The evidence does not tick the adoption wizard (CG6, line 65), and ticks neither `just verify-08` nor the design owner's approval.
- Spec chain: `514c48a 08/plan: approved by a human` exists (the same author as the agent commits; the human's approval is recorded in `blockers.md:323-326`, not provable from git); no later commit touches `plans/08-frontend.md`; task commits follow `08/T0` to `08/T6`. Extra-paths are untouched by the follow-up; `Contract-change` stays `needed` (the human approved without `--contract-change`) and nothing contract-side moved.
- Security trace, unchanged since round 1 and exercised again: S2 `auth/session.tsx` (`auth/session.test.tsx`, `lib/query/no-persist.test.ts`); S3 `api/client.ts:201-206` (`api/client.test.ts`; with the header line removed in a scratch copy 7 of 40 tests fail); S12 `lib/csp/trusted-types.ts`, `editor/policy.ts`, `features/docs/reader-html.ts`, `scripts/dist-check.mjs` (`e2e/csp/*.spec.ts`, `xss.spec.ts`); S14b `components/masking/plan.ts` (failing open in a scratch copy fails 5 of 72 tests); S21 `lint/lanekeeper-rules.mjs` (`lint/rules.test.mjs`). Threat model updated: yes, "Web app (08)" (not touched by the follow-up).

## Gates re-run

| Command | Result |
|---|---|
| `pnpm install --frozen-lockfile` | exit 0, up to date, 1.5 s |
| `pnpm lint` | exit 0, 100 s |
| `pnpm typecheck` | exit 0, 45 s |
| `pnpm exec vitest run` | 391 files, 9,683 tests passed, exit 0, 364 s (equals the evidence) |
| `pnpm exec vitest run --project node` | 164 files, 6,210 tests passed (equals the evidence) |
| `pnpm gen:api` then `git status --porcelain` | exit 0, 0 changed paths |
| `pnpm build` | exit 0, 60 s, initial JavaScript 196.0 KB gzip (195.982 in the result file), dist-check 158 files clean |
| `diff -rq` of this `dist` and the `e5ed17e` build | identical, 162 files |
| `pnpm build:perf` | exit 0, 196.6 KB, dist-check 172 files |
| `pnpm audit` | exit 1, 7 (5 moderate, 2 low); the `e5ed17e` tree: 29 (10 high, 17 moderate, 2 low) |
| S19 registry sweep | 770 of 770 versions at least 14.98 days old; the 17 added integrities match |
| `scripts/check-actions-pinned.sh` | All actions pinned by SHA. |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --locked -p hub --example web_fixture -- -D warnings` | exit 0 |
| `cargo test --locked -p xtask --test budgets_registry` | 8 passed |
| `playwright --project=perf --workers=1` (3 specs), `scripts/perf/lighthouse.mjs`, then `cargo xtask bench-check` on the five P10 ids | PASS 195.982 kb (max 250); PASS 54.8 ms p95 (100); PASS 83.7 ms p95 (500); PASS 304.5 fps min (60; vsync-locked 59.8, informational); PASS 93 p50 (90; Compare 95, file diff 93, Swimlanes 95); `5 pass, 0 fail, 0 unmet of 5 budgets` |
| `cargo xtask bench-check` (whole registry) | exit 1: 5 pass, 68 fail ("registered but no result"), 2 unmet of 75; the rest are other prompts' Rust benchmarks, not produced here |
| Playwright `e2e/csp/{csp,requests,assets,shell,overlays,clipboard}`, `e2e/editor/csp`, `e2e/keyboard/keymap` | 305 passed, 1.5 min |
| Playwright `e2e/csp/xss.spec.ts`; `-g "file edit"` a11y | 49 passed; 16 passed |
| Playwright `-g "tier 1" e2e/visual` (React 18.3.1 files checked against the sha512 pins) | 103 passed, 2.1 min |
| Playwright `e2e/visual` (744 tests, 684 baselines) | 743 passed, 1 failed (served S22 light), 13.5 min; S22 alone x5: 5 passed |
| Playwright `attribution.spec.ts:238` x120, `--workers=3` | 3 failed, 117 passed |
| Playwright `editor/csp.spec.ts:67` x60 under six busy loops | 60 passed |
| Playwright `--list` counts | 3,489 tests; every per-folder count in evidence line 31 matches |

## NOT re-run

- The full 3,489-test `chromium` project (68 minutes). I ran the csp folder, the editor spec, the keymap spec, the XSS corpus, the a11y file-edit cases, the perf project, tier-1 and the whole visual folder. I did not run the rest of a11y (1,092 listed), layout (1,025), journeys (148), the other keyboard specs (104), live (20) and harness (6).
- `just verify-08` and `just verify` as recipes (`just` is not installed), `cargo deny`, `cargo audit`, `osv-scanner` (`api.osv.dev` answers 403 through the sandbox proxy; `pnpm audit` and the registry sweep stand in), Docker.
- Anything on GitHub: the five shards, the `perf` job on a shared runner, the baseline label job, branch protection. The CI results for main are relayed by the evidence and not seen by me.
- Chromium 153 (CI's browser; mine is 141); the other prompts' Rust benchmarks; a human look at the 684 baseline images.

# Round 3

Reviewer: independent `reviewer` agent, fresh context, read-only. Reviewed branch `agent/08-frontend` at `fed5a5c` (follow-up commits `44d92da`, `7d69356`, `e082335`, `674b865`, `9440900`, `d13a957`, `01869a4`, `fed5a5c` after `e5ed17e`) against `prompts/REVIEW.md` and the `lanekeeper-pr-review` skill, on 2026-11-02 (15:05Z to 15:50Z). Nothing was edited, staged or committed; build output went to ignored folders and the scratchpad.

VERDICT: CHANGES REQUESTED

Every agent-side finding of round 2 is closed in code. One blocking item is left, and it needs a human, not another agent change: a committed baseline picture is wrong, so the tip's `web-e2e` is red on exactly one test.

## Blocking

1. `web/e2e/visual/__baselines__/visual/served/served.spec.ts/served-S22-light.png` (committed by the bot in `94bf740`), against the frame in `web/e2e/visual/served/frames.ts:107-118`: the tip is deterministically red on one visual test, so the gate of acceptance 1 (`just verify-08` runs `just web-e2e` with `retries: 0`) is not green.
   - The light baseline of frame S22 was photographed while the live pill was mid-reconnection (the pill mask is 180 px wide, x 1244-1423). The fixed frame keeps the stream alive and photographs the pill as "Live" (56 px, x 1368-1423, the same as the dark baseline). So S22 (light) differs by 6,694 pixels, which is 0.5165 percent of the frame against a limit of 0.5, and S22 (dark) passes. The two committed baselines of one frame disagree about the app's state.
   - This is not an agent code defect. The implementer's fix for R2-B1 (b) is right, it refused to regenerate the picture locally or loosen the limit, and it left acceptance 1 unticked (evidence line 59). But R2-B1 asked for the gate to be reliably green on the committed tree. It is now reliably red on this one test, and only the label-triggered runner job can change it.
   - Proof: `pnpm exec playwright test --project=chromium --workers=3 --repeat-each=6 -g S22 e2e/visual/served/served.spec.ts` gave `6 failed, 6 passed`: all 6 light runs `6694 pixels (ratio 0.01 of all image pixels) are different`, all 6 dark runs pass. The whole folder `e2e/visual/served` gave `1 failed, 45 passed` (the failure is S22 light). Magenta mask width in the header band (python3 -I, PIL): baseline light 180 px, baseline dark 56 px, my fresh light capture (`test-results/.../served-S22-light-actual.png`) 56 px. The margin is only 3 percent over the limit, so another renderer could pass it by accident while the baseline still records the wrong state.
   - Red-green for the fix itself, in a scratch copy: with the heartbeat wait removed from `aMinuteLater`, `e2e/visual/served/minute.spec.ts` fails 5 of 5; as committed it passes 15 of 15.
   - Fix, a human step (`.github/workflows/web-e2e.yml` line 59: "To refresh one, delete its PNG in a commit and add the label again"): delete `served-S22-light.png` in a commit on the branch, add the label `bootstrap-baselines`, look at the new PNG (pill masked and 56 px wide, switcher at about x 838-1103, banner "This view may be out of date"), then run `web-e2e`. Or the human records in writing that this red is accepted until then. This finding closes when that picture is replaced and the folder is green.

## Non-blocking

1. `pnpm audit` is red: 7 findings (5 moderate, 2 low, 0 high, 0 critical), run here at 15:10Z; `e5ed17e` had 29 (10 high), round 1 had 17 (1 critical). Left: `fast-uri` 3.1.8 (4), `vite` 6.4.4 (1, dev server), `minimatch` (1, two paths). Each fix is under 14 days old by the registry clock now: `fast-uri` 3.1.9 is 13.67 days (eligible 2026-11-02T23:05:26Z, about 8 hours away), `vite` 6.4.5 is 12.40 (2026-11-04T05:35:48Z), `minimatch` 11.0.1 is 5.70 (2026-11-10T22:26:03Z). Nothing eligible is left unbumped. The human decision in `osv-scanner.toml:10-13` (2026-10-26) names dompurify, undici, fast-uri, @vitest/mocker and zod; it does not name `vite` or `minimatch` (its fix was published on 10-27). The `osv` job of `web.yml` stays red until those dates. Merge precondition for a human: wait and re-resolve the lockfile after each date, or record an exception in `docs/decisions.md`.
2. D129 to D133 (`docs/decisions.md:382-412`) are still "PROPOSED, not yet decided". The evidence's "Open human decisions that stand" (lines 161-167) has no mention of them (0 matches in the file), and line 152 says W20 was "decided by the human on 2026-10-17", which `docs/decisions.md` does not show. Unchanged since round 2 non-blocking 2. Fix: list the five with their status, here and in the PR text.
3. The adoption wizard is still a `StubPage` (`web/src/features/adoption/AdoptPage.tsx:7-8`); `api/openapi.yaml` has no adoption operation (CG6). The evidence leaves it unticked (line 65). It needs the human's explicit acceptance of the deferral (on record in the blockers file) or `contract/08-adoption`.
4. `design/DEVIATIONS.md`: 179 of 179 rows are `pending`, so acceptance 4 stays unticked (evidence line 64). The follow-up edit `7d69356` is wording only (1 insertion, 1 deletion, last cell still `pending`), and its claims hold: `src/features/shell/crumbs.ts:40,75` cuts above nine parts with an ellipsis, `current-swimlane.ts:5` keeps the last swimlane, `e2e/visual/file/masks.ts` masks x 237, y 9, 619 by 25.
5. `plans/08-frontend.evidence.md:63` ticks "Visual regression and keyboard-map tests pass" and quotes "all 744 e2e/visual tests passed", which is true of the earlier tree only; at the tip the visual folder is 744 of 745 (blocking 1). Line 59 says so; line 63 should carry the same caveat. The header (line 3) still names HEAD `7d69356`, with an exception sentence for the round-2 part.
6. `docs/threat-model.md:846` and evidence line 77 say direct dependencies are "pinned exactly" / "exact pins in web/package.json". 11 of 43 are ranges (`react`, `react-dom`, `@types/react`, `@types/react-dom`, `@eslint/js`, `eslint`, `eslint-plugin-react`, `@vitejs/plugin-react`, `typescript-eslint`, `vite` with `^`; `typescript` with `~`), unchanged since `e5ed17e`. The real pin is the lockfile under `--frozen-lockfile` plus `minimumReleaseAge` (770 of 770 locked versions at least 15.14 days old), so this is wording. The same line still says "no known vulnerabilities at the T1 commit" beside 7 findings (round 2 non-blocking 8).
7. `@tailwindcss/node` 4.3.3 (`web/package.json:37`) is used only by `src/styles/tailwind.theme.test.ts:7` and nothing justifies it (rule 15). The PR text must carry it.
8. The frozen plan's traceability table names about 30 proof files that no longer exist (for example `lint/rules.test.ts` is `lint/rules.test.mjs`; `features/adoption/Wizard.test.tsx` has no substitute). The evidence maps only the S# lines and `plans/08-frontend.pr.md` does not exist yet; a full name-to-name table belongs in the PR text.
9. `web/e2e/visual/served/minute.spec.ts:41-45` watches the pill for 2 s with 20 steps of `waitForTimeout(100)`. It is a negative-assertion window (a stall shows late) and it tests the behaviour (5 of 5 red without the fix), so only noted against the no-sleeps rule.
10. `git diff e5ed17e..HEAD --check`: trailing whitespace at `plans/08-frontend.blockers.md:1384`. Cosmetic.
11. Thin margins, measured here alone: Lighthouse lowest page 93 against 90 (file diff 92, 93, 93), route change p95 59.2 ms of 100, diff of 6,000 lines p95 84.8 ms of 500, grid capacity 288.2 fps against a floor of 60, initial JavaScript 195.982 KB of 250. The CI `perf` job runs on a noisier runner and has no number yet; Lighthouse is the budget most likely to show it.

## Round-1 findings

| ID | Closed | How |
|---|---|---|
| B1 audit, S19 | No (reduced) | `pnpm audit`: 17 in round 1 (1 critical, 5 high), 29 (10 high) at `e5ed17e`, 7 now (0 high). Bump `44d92da` took every fix that is 14 days old; the three left are 13.67, 12.40 and 5.70 days old. S19 sweep at 15:07Z: 770 of 770 locked versions at least 15.14 days old, integrities equal the registry. Non-blocking 1. |
| B2 P10 not in CI | Yes | `.github/workflows/web-e2e.yml:174-209`: `perf` job runs `just web-perf` and `just web-bench-check`, no `continue-on-error`. Five budgets re-run here: 5 pass. No CI number seen. |
| B3 XSS corpus | Yes | `e2e/csp/xss.spec.ts` (574 lines) and `xss-corpus.ts` (379); the whole `e2e/csp` folder ran here: 328 passed, 49 of them the XSS tests. |
| B4 adoption wizard | No | Still a stub; CG6 not in the contract; unticked in the evidence. Non-blocking 3. |
| B5 relaxations | No | D129 to D133 are PROPOSED, not decided; the evidence does not list them. Non-blocking 2. |
| B6 gates not run | No (sandbox) | `just`, `cargo deny`, `cargo audit`, `osv-scanner`, Docker are not available here. Every cheap step of `verify-08` was run as a command and passed, except `web-e2e`, which has the one red test of blocking 1. |
| NB1 copy-paste | Yes | `isDocPath` has one definition (`features/docs/doc-path.ts:14`); whole vitest passes. |
| NB2 dead exports | Yes | `SignOutButton`, `swimlanePickupQuery`, `notificationCounts`, `hitSegments`, `VersionChip`, `RoleBadge` no longer exist; `differingCells` is only in `compare/model.test.ts`; the second `pickupVariant` is gone. |
| NB3 lint loopholes | Yes | `lint/rules.test.mjs` has `window.eval`, `globalThis.eval`, `setHTMLUnsafe` (and the bracket form), `createElementNS` script/iframe and `globalThis['Function']` cases; `pnpm lint` exit 0; the node project passes. |
| NB4 secret mutations | Yes | `src/lib/query/secret-usage.test.ts` exists and passes. |
| NB5 CI sequencing | Yes | 684 baselines committed by the bot (`94bf740`, an ancestor of `e5ed17e`), untouched by the follow-up. Caveat: S22 light is wrong (blocking 1). |
| NB6 intermittent failures | Yes | `scripts/perf/remove-dir.mjs` with its test; Lighthouse ran to the end (exit 0); a11y `-g "file edit"` 16 of 16. |
| NB7 acceptance 4 | No | 179 of 179 pending. Non-blocking 4. |
| NB8 traceability names | No (partial) | Substitutes exist for the rows sampled; no full map yet. Non-blocking 8. |
| NB9 dependencies | No (partial) | `@tailwindcss/node` unjustified. Non-blocking 7. |
| NB10 non-task commits | Yes | T0 to T6 in order (`cc3448c` to `9c9243a`); all eight follow-up commits end with the two required lines; no commit after `514c48a` touches `plans/08-frontend.md`; the plans files only gain lines (blockers 135/0, evidence 178/0, review 93/0). |

## Round-2 finding R2-B1 (the three flakes)

| Site | Closed | How |
|---|---|---|
| (a) `attribution.spec.ts:238` | Yes | `e2e/fixtures/test.ts`: the `harness` fixture depends on `page` and calls `unrouteAll({ behavior: "ignoreErrors" })` on page and context before it closes the harness; an auto fixture does the same for tests without it. With the old fixture in a scratch copy `e2e/harness/teardown.spec.ts` fails 20 of 20 (`route.fetch: aborted`); with the new one 20 of 20 pass. The attribution test: 120 of 120 pass with the command that gave 3 failed of 120 in round 2. |
| (b) S22 frame | Code yes; gate no | `frames.ts`: four jumps of 15.25 s with a heartbeat each and a wait until the page has read it; `minute.spec.ts` red 5 of 5 without the wait, 15 of 15 green with it. The picture is now "Live" in both themes, but the committed light baseline is the old wrong state: blocking 1. |
| (c) `editor/csp.spec.ts:80` | Yes | `settledFidelity` helper, DOM check on the typed text. With the wait removed in a scratch copy the new test fails 5 of 5 with `worst 19` (main's CI number); as committed the editor spec passes 110 of 110 (11 tests x 10). |

## Specific checks asked for

- S19: 17 versions added and 18 removed in the lockfile since `e5ed17e` (the same set as the evidence). Registry publish times at 15:07Z: the youngest is `seroval` 1.6.11 at 15.14 days (14.82 days at the bump commit's author time, 07:32Z). All 770 locked versions are at least 15.14 days old; all integrities equal the registry; no tarball or git source; `minimumReleaseAge: 20160` unchanged; the one new override (`@scalar/json-magic>undici: 7.30.1`) is scoped and explained.
- S12 and the CSP unchanged, no new egress or eval: no path under `crates/`, `web/src`, `web/lint`, `web/scripts`, `web/index.html` or `vite.config.ts` changed. I built the `e5ed17e` tree with its own lockfile and the tip: `diff -rq` of the two `dist` folders is identical (162 files). `dist-check` scanned 158 files, nothing the CSP would refuse; the 328 csp tests (49 XSS, the requests and clipboard specs among them) and the editor CSP spec pass. The follow-up's test code has no `.only`, `.skip`, `fixme`, `eval`, `new Function` or off-site URL.
- The 684 app-owned baselines (593 was round 1's count of skipped tests): untouched by the follow-up, 0 paths. The DEVIATIONS edit is wording only and the row stays `pending`. The evidence does not tick the adoption wizard (line 65), the design owner's approval (line 64) or `just verify-08` (line 59).
- The follow-up touched no contract path and not `docs/decisions.md` (0 files); the five P10 entries of `perf/budgets.toml` are unchanged. Extra-paths are untouched by the follow-up; `Contract-change` stays `needed` as recorded (the human approved without `--contract-change`).
- Spec chain: `514c48a 08/plan: approved by a human` exists; no later commit touches the plan; task commits follow `08/T0` to `08/T6`.
- Security trace, unchanged since round 1 and exercised again in the whole vitest run and the csp folder: S2 `auth/session.tsx`; S3 `api/client.ts` (`api/client.test.ts`); S12 `lib/csp/trusted-types.ts`, `editor/policy.ts`, `features/docs/reader-html.ts`, `scripts/dist-check.mjs` (`e2e/csp/*.spec.ts`, `xss.spec.ts`); S14b `components/masking/plan.ts`; S21 `lint/lanekeeper-rules.mjs` (`lint/rules.test.mjs`); S19 above. Threat model updated: yes ("Web app (08)"; not touched by the follow-up).

## Gates re-run

| Command | Result |
|---|---|
| `pnpm install --frozen-lockfile` | exit 0, up to date, 1.2 s |
| `pnpm lint` | exit 0, 95 s |
| `pnpm typecheck` | exit 0 |
| `pnpm exec vitest run` | 391 files, 9,683 tests passed, exit 0, 392 s (equals the evidence) |
| `pnpm gen:api`, then `git status --porcelain` | exit 0, 0 changed paths |
| `pnpm build` | exit 0, 68 s; initial JavaScript 196.0 KB gzip of 250; dist-check 158 files clean |
| `pnpm build:perf` | exit 0, 23 s; 196.6 KB; dist-check 172 files |
| `dist` of the `e5ed17e` tree (own lockfile) against the tip | identical, 162 files |
| `pnpm audit` | exit 1, 7 (5 moderate, 2 low) |
| S19 registry sweep, 770 versions | 0 under 14 days (youngest 15.14), 0 integrity mismatches, 0 tarball or git sources |
| `scripts/check-actions-pinned.sh` | All actions pinned by SHA |
| `cargo fmt --all -- --check` | exit 0 |
| Playwright attribution test x120, 3 workers | 120 passed |
| Playwright `harness/teardown.spec.ts` x20, old fixture / new fixture | 20 failed / 20 passed |
| scratch mutations: `settledFidelity` without waiting; heartbeat wait removed | 5 of 5 fail / 5 of 5 fail |
| Playwright `minute.spec.ts` x15; `editor/csp.spec.ts` x10 | 15 passed; 110 passed |
| Playwright S22 x6; whole `e2e/visual/served` | light 0 of 6, dark 6 of 6; folder 45 passed, 1 failed (S22 light) |
| Playwright `e2e/csp` + `keyboard/keymap.spec.ts` | 344 passed, 3.9 min |
| Playwright a11y `-g "file edit"` | 16 passed |
| Playwright `--list` | 3,495 listed (3 are perf-project timing specs); every per-folder count equals the evidence |
| Playwright `--project=perf --workers=1`; `scripts/perf/lighthouse.mjs` | 3 passed, 2.6 min; Lighthouse exit 0, lowest page 93 (Compare 95, file diff 93, Swimlanes 95) |
| `cargo xtask bench-check` on the five P10 ids (the `web-bench-check` body) | PASS 195.982 kb (250); PASS 59.2 ms p95 (100); PASS 84.8 ms p95 (500); PASS 288.2 fps min (60); PASS 93 p50 (90); 5 pass, 0 fail, 0 unmet |
| `cargo xtask bench-check` (whole registry) | exit 1: 5 pass, 68 fail ("registered but no result"), 2 unmet of 75; the rest are other prompts' Rust benchmarks |

## NOT re-run

- The full 3,492-test `chromium` project (about 71 minutes). I ran the csp folder, the keymap spec, the editor spec x10, the a11y file-edit cases, the teardown and minute specs, the whole served visual folder, the attribution test x120, the perf project and Lighthouse. I did not run the rest of a11y (1,076 of 1,092), layout (1,025), the rest of journeys, the other keyboard specs (88), live (20) and the other visual folders (699 of 745, including tier-1, which needs `LK_REACT18_DIR`).
- `just verify-08` and `just verify` as recipes (`just` is not installed), `cargo deny`, `cargo audit`, `osv-scanner`, Docker; `cargo clippy` and the xtask `budgets_registry` test (no Rust or budget path changed after `e5ed17e`).
- Anything on GitHub: the five shards, the `perf` job on a shared runner, the baseline label job, branch protection. The CI results for main are relayed by the evidence and not seen by me.
- Chromium 153 (CI's browser; mine is 141); the other prompts' Rust benchmarks; a human look at the 684 baseline images.
