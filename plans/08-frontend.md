# Plan 08: Web app

Status: DRAFT
Contract-change: needed
Extra-paths: `crates/hub/examples/web_fixture.rs` `.github/workflows/web-e2e.yml` `docs/threat-model.md`
Prompt: prompts/08-frontend.md
Branch: agent/08-frontend

## Docs read

- `AGENTS.md`, `docs/decisions.md` (D2, D31, D33 to D38, D43 to D48, D66, D68, D71, D73, D76, D79 to D81, D85 to D87, D107, D109), `docs/security.md` (S2, S3, S12, S14b, S19, S21, S24), `docs/performance.md` (P10, P13, mechanisms 6, 10, 11), `docs/interfaces.md` (DomainEvent list, WriteOutcome, ChangePreview), `docs/domain-model.md` (drift labels, pickup states, PR jobs), `docs/open-questions.md` (Q35 blocks T6, Q37), `prompts/08-frontend.md`. Prompts 01, 03a, 05, 06 and 14 are named by the prompt only as integration order; not read.
- Skills: `lanekeeper-ui-from-design`, `lanekeeper-security`, `lanekeeper-performance`, `lanekeeper-contract-change`, `lanekeeper-config-semantics`, vendored `shadcn` (CLI pinned to `shadcn@4.21.0` in `THIRD-PARTY.md`) and `webapp-testing`.
- Handoff: `design/handoff/screens/README.md` (the bundle README is there; `design/handoff/README.md` does not exist), `Lanekeeper Engineering Handoff.dc.html`, `Lanekeeper States Index.dc.html`, the `data-props` of every `.dc.html`, `support.js`, `design/06`, `design/13`, `design/14`.
- Code at base commit 777b590: `crates/hub/src/{headers,static_files,config,compose}.rs`, `crates/hub/Dockerfile`, `crates/hub-identity/src/{csrf.rs,session/cookie.rs,guard/problem.rs}`, `crates/hub-platform/src/events/{sse,source,wire,visibility}.rs`, `crates/ports/src/events.rs`, `crates/domain/src/{events,compare,findings}.rs`, `crates/hub-writes/src/routes/{problem,me}.rs`, `api/openapi.yaml` (91 operations), `perf/budgets.toml`, `crates/xtask/src/bench_check.rs`, `Justfile`, `scripts/lk-lib.mjs`, the `web/` scaffold. The `monaco-editor@0.57.0` and `msw@3.0.1` tarballs were read statically (nothing executed).

### Verified facts that bind the tasks

- **F1 The hub serves `web/dist` itself** (`static_files.rs`, `Dockerfile`).
  - `index.html` is read once at start-up and the hub refuses to start unless it contains `__CSP_NONCE__` (`StaticError::NoPlaceholder`). The scaffold `index.html` has none, so a scaffold build cannot be served.
  - Every occurrence of the placeholder is replaced by that response's nonce.
  - `/assets/*` is `immutable` (needs hashed names, Vite's default). Everything else is `no-store`, including `public/` files, so fonts must be imported from `src/` to land in `/assets/`.
  - Path segments are `[A-Za-z0-9._-]`, at most 128 characters, no symlinks, files up to 32 MiB. Extension-less paths get `index.html` (client routes). There is no `Content-Encoding`: the P10 gzip figure is measured, not served.
  - The image's web stage copies only `web/` and runs `pnpm install --frozen-lockfile && pnpm build`. `pnpm build` therefore cannot read `../api` or `../design`: the generated API client and `tokens.css` are committed, and the bundle gate runs inside `pnpm build` (it then also runs in the existing CI `web` job and in the image build, with no workflow edit).
- **F2 Production CSP** (`headers.rs`, `config.rs`).
  - Directives: `default-src 'none'; script-src 'self' 'nonce-N'; style-src 'self' 'nonce-N'; img-src 'self' data:; font-src 'self'; connect-src 'self'; worker-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; require-trusted-types-for 'script'; trusted-types lanekeeper-docs lanekeeper-monaco`.
  - The two names are the default of `LK_TRUSTED_TYPES`; no deploy file sets it. There is no `allow-duplicates`.
  - Consequences: no `blob:` or `data:` workers or fonts; no `style=""` markup or `setAttribute('style')` (React `style` props and `el.style.x` are CSSOM and fine); runtime `<style>` elements need the nonce; at most two policy names, each created once; no `eval`; the string forms of `Worker()` and `register()` are Trusted Types sinks.
  - Static assets, and therefore worker scripts, carry the API CSP (`default-src 'none'`).
- **F3 CSRF** (`csrf.rs`, `cookie.rs`). `CsrfLayer` wants exactly one `Origin` equal to the public origin, exactly one `X-CSRF-Token` header, and exactly one `__Host-lk_csrf` cookie with the same value. The cookie is Secure, SameSite=Lax and not HttpOnly, so the page reads it from `document.cookie`. A refusal is always the same 403 (`urn:lanekeeper:problem:csrf`).
  - **A `<form method=post>` navigation sends `Origin: null` under `Referrer-Policy: no-referrer`**, so every non-GET is a `fetch` (default mode `cors`, which sends the real Origin). Sign-out is `POST /auth/logout` by `fetch`. Sign-in is a GET navigation to `/auth/login?return_to=`.
  - The hub does not compose `CsrfLayer` yet (D109 D-2). The client sends the header regardless.
- **F4 SSE** (`sse.rs`, `source.rs`, `wire.rs`).
  - `GET /api/v1/events`. Only the `Last-Event-ID` header is read; there is no query form. A fresh browser `EventSource` object never sends it, so the client is a `fetch` stream.
  - Frames: `retry: 3000` first, a replay or a `resync`, then `id: N`, `event: <kind>`, `data: <json>`, and `: heartbeat` every 15 s. `resync` has no `id` (the last id must not move).
  - Limits: 5 streams per user (429 beyond), 30 minute maximum age (the client reconnects with its id and loses nothing), replay at most 1,000 events (else `resync`). Internal events are dropped at the edge. `access_request_created` is Admin-only. A role change takes effect at reconnect.
- **F5 API responses are `no-store`** (`headers.rs` `apply`; only assets and blobs may be `immutable`). The browser never revalidates them, so "requests use ETags" (T2) means the app's own fetch layer sends `If-None-Match` and keeps bodies. Hash-keyed content needs no revalidation.
- **F6 Problems.** `type` is `urn:lanekeeper:problem:<code>`; write routes also send `code`. Seen: `unauthenticated`, `access-pending`, `access-disabled`, `insufficient-role`, `csrf`, `not-found`, `classic_token`, `not_fine_grained_token`, `token_rejected`, `token_missing_access` (with `base` and `tenant` booleans), `github_account_mismatch`, `token_expired`, `token_unreadable`, `idempotency_conflict`, `locked` (with `until`), conflict (with `current`, or with `reason: duplicate_pr` and `link`). `Problem` in OpenAPI documents none of these codes. The UI branches on `type`.
- **F7 Monaco 0.57.0** (read from the tarball).
  - Policies it creates: `defaultWorkerFactory`, `diffEditorWidget`, `diffReview`, `domLineBreaksComputer`, `editorGhostText`, `editorViewLayer`, `richScreenReaderContent`, `standaloneColorizer`, `stickyScrollViewLayer`, `tokenizeToString`, `domSanitize<N>`. That is 11 or more names against 2 allowed.
  - They are all created through `createTrustedTypesPolicy`, which calls `MonacoEnvironment.createTrustedTypesPolicy(name, options)` when set, so one allowed policy can serve them all.
  - Exceptions: its vendored DOMPurify calls `trustedTypes.createPolicy` directly (guarded by try/catch, but a disallowed name still fires a `trusted-types` violation report), and `webWorkerServiceImpl.js:95` has a string bootstrap that is used only when no `getWorker` is supplied.
  - `createStyleSheet` makes `<style>` elements with no nonce.
  - `DiffEditor` sets an inline `style` attribute (`diffEditorViewZones.js:199`), which the CSP blocks. We do not use it: diffs are server hunks.
  - `monaco-editor` depends on `marked` and `dompurify`; both stay inside the lazy chunk.
- **F8 The `.dc.html` references.**
  - `support.js` fetches React 18.3.1, ReactDOM 18.3.1 and `@babel/standalone` 7.29.0 from `unpkg.com` at run time and compiles inline `text/x-dc` scripts with Babel (needs inline script and eval). They render only in a sandboxed context. Only `theme` can be set from the URL (`?theme=dark`); other props are defaults in `data-props`.
  - The page files have `$preview` sizes of 1216x856 (the content area of a 1440x900 shell: 1440 minus 224 sidebar, 900 minus 44 header). `Shell` is 1440x900 with a `page` prop for nine pages. `Access Screen` is 1280x800.
  - **The States index has 67 rows; 17 are "wired" in a full-size page file and 50 are "specimen only" 600x380 frames** (`States.dc.html` `.fr`), which no 1440x900 app screenshot can match pixel for pixel.
  - The handoff contradicts itself twice: two shortcut sets (Style Guide J/K, X, [ ] versus Shell arrows, n/p, u; it follows Shell), and route `state=withdrawn` for approvals while the page file and `ProposalStatus` have `pending, stale, expired, decided`.
- **F9 Sandbox.** No Chromium is installed in this agent sandbox and `cdn.playwright.dev` answers 403 through the proxy; the npm registry works. Node 22.22 and pnpm 10.34.6 are present. Browser gates (Playwright, Lighthouse) therefore cannot run here.
- **F10 Contract gaps** are listed with proposed shapes under "Extra paths and contract changes" (C1 to C9). The largest: no `DomainEvent` payload schema (the SSE response is `type: string`), no adoption operations, no sanitised docs HTML, and `SwimlaneSummary` lacks tier, baseline state, pending-service count and syncing.
- **F11 Units.** `InlineRange` and `TextHit.ranges` are UTF-8 byte offsets (domain comments; OpenAPI is silent). JavaScript strings are UTF-16, so the diff and search views convert per line.
- **F12 Stop conditions checked now.** No handoff design needs an inline script, an external font or a third-party asset (the handoff's own CSP table agrees; the only external URLs are the three in the reference runtime `support.js`, which is not shipped); every virtualised list has a fixed row height. The Monaco and bundle-size conditions are gated in T1.

## Tasks and commits

Commit order is the prompt's order, one commit per task. Fan-out rules for every task:

- Slices are sub-agents on disjoint paths. They never run `pnpm install`, `pnpm build` or `pnpm gen:api`, and never edit a shared file. They run only `pnpm exec vitest run <own dir>` and `pnpm exec eslint <own dir>`.
- Only the task's implementer edits shared files, runs install, generation, typecheck and build, runs the task's `Verify`, and makes the one commit.
- Shared files, in every task: `web/package.json`, `web/pnpm-lock.yaml`, `web/vite.config.ts`, `web/tsconfig.json`, `web/eslint.config.js`, `web/playwright.config.ts`, `web/orval.config.ts`, `web/src/router.tsx`, `web/src/routes/index.ts`, `web/src/lib/query/keys.ts`, `web/src/api/generated/**`, `web/src/styles/tokens.css`, `web/src/styles/tailwind.css`, `Justfile`, `design/DEVIATIONS.md`.
- Graphify reads the index of 777b590. It shows the Rust and OpenAPI side only, never the implementers' `web/` changes (use `Read` for those).
- Gating: T3 needs C1 (the event schema) merged first; T4 slices S2, S3, S5, S6, S7 and T6 need the contract items named under them. A gated slice waits; nobody hand-types an API type or works around a gap.

| Task | Commit message | Files and types created or changed | Verify (from the prompt) |
|---|---|---|---|
| T0 | `08/T0: import the design handoff (tokens, fonts, icons, shadcn map, handoff extractors)` | `web/scripts/handoff/*`, `web/src/handoff/*.generated.ts`, `web/src/styles/{tokens,tailwind,fonts,base}.css`, `web/src/assets/fonts/*`, `web/src/components/{ui,app}/*`, `web/components.json`, `design/DEVIATIONS.md` (seed) | token parity test |
| T1 | `08/T1: foundation and S12 (build, CSP and Trusted Types, Monaco gate, API client, lint, CSP e2e)` | `web/index.html`, `web/public/theme-init.js`, `web/vite.config.ts`, `web/src/lib/{csp,csrf}/*`, `web/src/editor/*`, `web/orval.config.ts`, `web/src/api/*`, `web/src/mocks/*`, `web/lint/*`, `web/src/{router,auth}/*`, `web/playwright.config.ts`, `web/e2e/{harness,csp}/*`, `crates/hub/examples/web_fixture.rs`, `.github/workflows/web-e2e.yml`, `Justfile` (`verify-08`, `web-e2e`) | lint ban works; CSP violation test |
| T2 | `08/T2: performance (code splitting, lazy Monaco, bundle gate, ETags, virtualised views, P10 gates)` | `web/src/lib/query/*`, `web/src/lib/text/byte-ranges.ts`, `web/src/components/app/{virtual,diff}/*`, `web/src/editor/loader.ts`, `web/scripts/perf/*`, `web/src/mocks/scenarios/*`, `web/e2e/perf/*`, `web/lighthouserc.json`, `perf/budgets.toml` (contract) | Lighthouse CI; Playwright performance tests; bundle gate |
| T3 | `08/T3: live updates (SSE client, backoff, Last-Event-ID, precise invalidation)` | `web/src/live/*`, `web/src/components/app/LiveIndicator.tsx`, `web/e2e/live/*` | mocked event stream test |
| T4 | `08/T4: screens (access, swimlanes, file view, compare, search and docs, findings and adoption, approvals and audit, admin, shell)` | `web/src/features/*`, `web/src/routes/*`, `web/src/keyboard/*`, `web/e2e/{visual,keyboard,journeys,a11y}/*`, `design/DEVIATIONS.md` | component tests, visual regression, keyboard maps, e2e smoke, axe |
| T5 | `08/T5: attribution, severity, PR links, masking and sentinels` | `web/src/components/app/{AttributionChip,SeverityBadge,PrBadge,MaskedValue}.tsx`, `web/src/features/{file,admin}/*` additions | component and visual tests; masked-values DOM test |
| T6 | `08/T6: pickup states, notify and as served` | `web/src/components/app/PickupChip.tsx`, `web/src/features/{swimlanes,file,admin}/*` additions | component and visual tests |

### T0: Import the design handoff

- **Tokens.** `tokens.css` holds `:root` and `.dark` (also `[data-theme="dark"]`), taken from the handoff `tokens/*.css`. A generator script writes it so the parity test compares two independent sources: the handoff CSS and the token tables inside the Engineering Handoff page. `tailwind.css` is Tailwind v4 CSS-first: an `@theme inline` block maps every semantic key to `var(--name)`, with `diff-empty` as a background image, plus `shadow-popover` and `overlay`.
- **Handoff data.** `acorn` parses the `<script type="text/x-dc">` of the Engineering Handoff and States Index pages with a small constant folder (literals, template strings, `+`, const identifiers, arrays). Nothing from the handoff is executed. Output is committed as `src/handoff/{tokens,routes,keymap,states}.generated.ts`, and `handoff-sync.test.ts` fails when the handoff changes without regenerating.
- **Fonts and icons.** The five woff2 files are byte copies of the handoff into `src/assets/fonts/` (so Vite emits them hashed under `/assets/`), with the OFL texts in `LICENSES.md`. Icons are `lucide-react` named imports; a test maps every handoff `assets/icons/*.svg` to a `lucide-react` export.
- **Components.** shadcn sources come from `pnpm dlx shadcn@4.21.0 add ...` (pinned by `THIRD-PARTY.md`), are reviewed, and are committed. `ScrollArea` and `sonner` are not used (see CSP notes below). `components/app/` holds the UI-vocabulary variants (`StatusBadge`: sync, git, nfs, conflict, intentional, unknown, untracked, restart, locked, approval; colour plus icon plus label; solid, dashed and outline weights). They are keyed by the handoff's names, not by API enums (the generated enums arrive in T1; the mapping `Record<DriftState, StatusVariant>` with `satisfies` is added there).
- **Fan-out.** All four slices are parallel after the implementer installs every dependency of T0 up front.
  - A tokens, Tailwind theme and contrast test: `src/styles/{tokens,tailwind,base}.css`, `scripts/handoff/extract-tokens.mjs`, `src/styles/*.test.ts`.
  - B fonts and icons: `src/assets/fonts/**`, `src/styles/fonts.css`, `src/components/icons/**`, tests.
  - C shadcn primitives and app variants: `src/components/**`, `components.json`.
  - D routes, keymap and States extractors: `scripts/handoff/{dc-parse,extract-routes,extract-keymap,extract-states}.mjs`, `src/handoff/**`, goldens.
  - Shared: `package.json`, lockfile, `vite.config.ts`, `tsconfig.json`, `eslint.config.js`, `web/.gitignore`, `design/DEVIATIONS.md`.
- **Graphify.** (1) Which OpenAPI enums carry status vocabulary (`DriftState`, `Severity`, `PickupState`, `Confidence`, `AttributionSource`, `PrState`, `PrJobState`, `ProposalStatus`, `AgentStatus`, `Role`, `FileClass`) so the variant maps are exhaustive. (2) The order of `FindingKind` (C1 to C12). The `.dc.html` files are not in the index; read them directly.
- **Tests.** `tokens.parity.test.ts`, `contrast.test.ts` (the handoff says the Style Guide's contrast check becomes a unit test; both themes, text pairs 4.5:1, ring and input 3:1), `fonts.test.ts` (sha256 against the handoff), `icons.test.ts`, `StatusBadge.test.tsx`, `handoff-sync.test.ts` and extractor goldens.

### T1: Foundation and security (S12)

- **Order inside the task.** The Monaco gate (slice B) runs first. If Monaco cannot run under this CSP and Trusted Types without an unsafe exception, stop and escalate (prompt rule) before any other slice is committed to a Monaco shape.
- **Build.** `index.html` is replaced: Vite `html.cspNonce: '__CSP_NONCE__'` writes the placeholder into every generated `nonce` attribute and a `<meta property="csp-nonce">`; a classic `<script src="/theme-init.js">` (same origin, no inline script) reads the `lk_theme` cookie and sets `class="dark"` before first paint. `vite.config.ts` sets `build.assetsInlineLimit: 0` (no `data:` fonts), `build.manifest: true` and a small bundle-report plugin (module ids per chunk), keeps the `/api` and `/auth` dev proxy and the `@` alias. A `dist.test.ts` scans `dist/` for external URLs, `eval`, `new Function`, `data:` fonts and a missing placeholder.
- **S12 code.** `src/lib/csp/nonce.ts` reads the nonce from the meta element's `.nonce` property and sets `window.__webpack_nonce__` (the modal Radix layers use `react-remove-scroll`, whose style singleton is expected to read that global; the overlay sweep confirms it). `src/lib/csp/trusted-types.ts` is the only module that calls `trustedTypes.createPolicy`: `lanekeeper-docs` (`createHTML`, used only by `SanitisedDocHtml`) and `lanekeeper-monaco` (`createHTML` and a `createScriptURL` that accepts only same-origin `/assets/*.js` URLs), each created once at module load.
- **Monaco plan.** See "Monaco under S12" below.
- **API client.** `orval.config.ts` reads `../api/openapi.yaml`, generates react-query hooks, types and MSW handlers into `src/api/generated/` (committed; a freshness check runs `pnpm gen:api && git diff --exit-code`). `src/api/client.ts` is the one mutator: same-origin `fetch`, `X-CSRF-Token` from the `__Host-lk_csrf` cookie on every non-GET, an `Idempotency-Key` (UUID per user attempt, reused on an automatic retry of the same attempt), an `AbortSignal` timeout on every request (SSE excepted), a bounded ETag cache (200 entries, 8 MiB) that sends `If-None-Match` and serves the kept body on 304, and `application/problem+json` parsed into an `ApiProblem` (`type` kind, `status`, `title`, `request_id`). 401 clears the query cache and navigates to `/auth/login?return_to=<own path>`; 403 routes by problem type (`access-pending`, `access-disabled`, `insufficient-role`, `csrf`).
- **Lint (S21 equivalents).** `web/lint/lanekeeper-rules.mjs` is a flat-config array imported by `eslint.config.js`: `strictTypeChecked` with `no-explicit-any`, `no-non-null-assertion` and `no-floating-promises`; `react/no-danger` with one file override (`SanitisedDocHtml.tsx`); bans on `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `eval`, `new Function`, string timers, `javascript:` URLs, `setAttribute('style')`, `trustedTypes.createPolicy` outside `lib/csp`, `localStorage` and `sessionStorage` outside one preferences wrapper (S2), `<form method="post">` (F3), the `onMutate` option and `setQueryData` inside mutations (handoff: writes are not optimistic), and direct `fetch('/api` outside the mutator; `jsx-a11y` and `react-hooks`. `lint/rules.test.ts` lints bad snippets with the ESLint API and asserts each rule fires.
- **Router and auth.** `src/router.tsx` builds code-based routes from `routes.generated.ts` with `minRole` metadata; a role gate renders the permission screen, not a redirect (handoff). `useMe` boots the session.
- **CSP test source.** See "How the CSP test gets the production policy" below.
- **Justfile.** `verify-08` replaces its stub: install, `gen:api` freshness, lint, typecheck, `vitest --run`, `pnpm build` (bundle gate), `just web-e2e` (builds the Rust fixture, runs Playwright; fails with install instructions when Chromium is missing and never skips), then `cargo xtask bench-check` on the five P10 ids using the filtered-budget pattern of `writes-bench`.
- **Fan-out.**
  - A (rust-implementer): `crates/hub/examples/web_fixture.rs` only.
  - B (web) CSP, Trusted Types and Monaco gate: `src/lib/csp/**`, `src/editor/**`, `e2e/csp/monaco.spec.ts`. Runs first; the other slices can start once it reports pass.
  - C (web) API client and mocks: `orval.config.ts`, `src/api/{client,problem,etag}.ts`, `src/lib/csrf/**`, `src/mocks/**`, tests (the implementer runs `gen:api` for it).
  - D (web) lint: `web/lint/**`, `lint/rules.test.ts`; the implementer imports it in `eslint.config.js`.
  - E (ops-implementer) `.github/workflows/web-e2e.yml` only (actions pinned by SHA, reusing the SHAs already in `ci.yml`).
  - F (web) e2e harness and CSP spec: `e2e/harness/**`, `e2e/csp/{csp,assets,requests}.spec.ts`, `playwright.config.ts` is shared. Starts after A and C.
  - Implementer only: router and auth, `index.html`, `vite.config.ts`, `Justfile`, install and generation.
  - Serial because: B gates the rest; F needs A and C; router and auth need C's problem types.
- **Graphify.** (1) `headers::{html_csp, security_headers, classify, HeaderPolicy}` (confirm no later change). (2) `StaticFiles::open`, `NONCE_PLACEHOLDER`, `safe_segments` (asset name rules). (3) `CSRF_HEADER`, `CSRF_COOKIE`, `check` and who calls `CsrfLayer::new` (D109 says nobody yet). (4) How `Hub::build` and `serve.rs` give a request its `PeerAddr`, and what `FakeSessionAuth` offers, so the example can serve `Hub::web` on loopback without identity.

### T2: Performance (P10)

- **Code splitting.** Every route is `lazyRouteComponent`; `defaultPreload: 'intent'` and a preload stale time make cached route changes synchronous. Monaco is a dynamic import behind `src/editor/loader.ts`, used only by Edit mode (plus an idle prefetch for Editor and above when the file view is interactive and `saveData` is off). The file viewer and the diff use `CodeView` and `DiffView` (virtualised, server hunks), not Monaco. This is what keeps the diff interactive in 500 ms. See W9.
- **Bundle gate.** `scripts/perf/check-bundle.mjs` runs at the end of `pnpm build`: initial JavaScript is the entry chunk plus its static import closure from `dist/.vite/manifest.json`, each file gzipped at level 9, summed, with 1 KB taken as 1,000 bytes (the stricter reading). It fails above 250 and prints a warning above 225. It also fails if the initial closure contains `monaco-editor`, `msw`, `@faker-js`, `cmdk`, `dompurify` or `marked`. It writes `target/perf-results/web-initial-js-gzip-kb.json` (`bench: web/initial_js_gzip_kb`, `unit: kb`, `statistic: max`).
- **Data.** `src/lib/query/keys.ts` is the full key vocabulary for every operation (see "Query keys and SSE" below); content, compare and git-by-commit keys carry hashes and are marked `meta.immutable`. Query defaults: bounded `gcTime`, no persister (S2, S10), `refetchOnWindowFocus: false`. The ETag cache from T1 is wired to the keys that need it (current content, compare, grid, served).
- **Rendering.** `VirtualList` (fixed heights 32, 28 tree, 28 compact, 18 diff; `overscan: 10`; `aria-rowcount` and `aria-rowindex`; the focused or selected row stays rendered), `DataTable` (div grid, `role=grid`, cursor "Load 100 more" keeping focus on the first new row), `TreeView` (`role=tree`), `CodeView`, `DiffView` (split and unified share one virtualiser; fold rows are built from the gaps between hunks; word highlights from `inline_changes`). `src/lib/text/byte-ranges.ts` converts UTF-8 byte ranges to UTF-16 indices with an ASCII fast path.
- **Perf specs** (Playwright, against the harness): `diff-6000.spec.ts`, `grid-5000.spec.ts`, `route-change.spec.ts` write `target/perf-results/web-*.json`; `lighthouserc.json` uses the desktop preset, 3 runs, median score times 100 into `web-lighthouse-performance.json`. The five P10 entries in `perf/budgets.toml` become `registered = true` (C9).
- **Fan-out.**
  - A `scripts/perf/**` (gate, result writers, Lighthouse runner).
  - B `src/lib/query/**` (client, ETag wiring, defaults) plus tests; `keys.ts` is shared and written by the implementer from the generated operation list.
  - C `src/components/app/virtual/**` (VirtualList, DataTable, TreeView).
  - D `src/components/app/diff/**` and `src/lib/text/**` (CodeView, DiffView, byte ranges).
  - E `src/mocks/scenarios/**` (seeded generators typed by the generated schemas: 6,000-line diff, 5,000-row grid, large tree) and `e2e/perf/**`, `lighthouserc.json`.
  - Implementer only: router splitting, the `build` script edit, `perf/budgets.toml`, `vite.config.ts`.
  - Serial because: the perf specs need C, D and E; router splitting needs A's gate to measure.
- **Graphify.** (1) `DiffHunk`, `DiffLine`, `InlineRange` and how the engine fills context and inline changes, to size fold rows and the byte-to-UTF-16 conversion. (2) `Limit` maximum 500 and `Cursor` semantics for page size. (3) Which read handlers set `ETag` and answer 304 (`getFileContent`, `compareFile`, `getGrid`, `getServedFile`, `readDoc`). (4) `GridRowPage` and `GridRow` shapes for the 5,000-row scenario.

### T3: Live updates (P13)

- **Transport.** `sse-client.ts` is a `fetch` stream (`Accept: text/event-stream`, `Last-Event-ID` when known, `cache: 'no-store'`, same-origin credentials) read through `TextDecoder` with a WHATWG-rules line parser, a 64 KiB frame cap (the server's events are at most 4 KiB), and a stall watchdog of 45 s without a byte (three heartbeats). `EventSource` is rejected: no header control on a fresh object, no backoff control, no status codes.
- **State machine.** `connecting`, `live`, `reconnecting` (attempt, retry time), `offline` after 3 consecutive failures, with retry continuing at the cap. Backoff 1, 2, 4, 8, 15 s, capped, with plus or minus 20% jitter; the server's `retry:` hint is the floor after a clean end. In `offline` the app shows the banner "Live updates paused, retrying", disables writes with the reason "Unavailable offline", and keeps reads. HTTP answers: 401 goes to sign-in, 403 stops and routes by problem type, 429 and 5xx back off, 400 stops with an error state (never a tight loop).
- **Resume.** The last id is kept in memory from frames that have an `id` only. A short blip relies on `Last-Event-ID` replay. After the client was `offline`, and on any `resync`, it refetches the active non-immutable queries (the handoff's "refetch the visible queries"). Routine 30 minute stream rotation never refetches. See W11 and the deviation row.
- **Invalidation.** `invalidation.ts` is a pure `plan(event)` over the generated `DomainEvent` with an exhaustive `switch` and a `never` check (a kind added to the contract fails to compile, like `is_internal_kind` in Rust); `apply()` coalesces over 100 ms, dedupes by key hash, falls back to "all active" above 200 distinct keys, and uses `refetchType: 'active'` so inactive queries are only marked stale. Lists follow the handoff's stable-view rule: values update in place when row keys and order are unchanged; otherwise an "n updates" chip applies the change on click (`useStableList`).
- **Fan-out.**
  - A `sse-parser.ts` plus property tests.
  - B `sse-client.ts`, `backoff.ts` and tests (interface agreed with A first).
  - C `invalidation.ts` and tests (needs the generated `DomainEvent` from C1 and the T2 keys).
  - D `LiveIndicator`, the paused banner, `useStableList`, the toast hook.
  - E `e2e/live/**` (harness `sse()` handlers).
  - Implementer only: `LiveProvider.tsx` mount, router root, `src/live/index.ts`.
  - Serial because: small; A, C and D in parallel, then B, then E.
- **Graphify.** (1) `DomainEvent` variants and `kind_name`: confirm the wire `type` strings. (2) `SseSettings::default` and any override in hub config (heartbeat, retry hint, per-user cap, max age, replay limit). (3) The `publish_in_tx` and `publish` call sites at the base commit: which kinds are actually published today. (4) `RoleVisibility` and `EventFilter::matches`.

### T4: Screens

T4 stays one commit (`08/T4:`), assembled by the implementer from the slices. It will be a very large diff; see W22.

- **Phase A, serial.** Slice S0 builds the shell and the state kits first, because every other slice mounts inside them. Phase B runs S1 to S8 in parallel. Phase C, serial, wires the visual matrix, keyboard and journey specs and finalises `DEVIATIONS.md`.
- **Slices.**

| Slice | Owns | Content | Gated by |
|---|---|---|---|
| S0 shell (first, serial) | `src/features/shell/**`, `src/keyboard/**`, `src/routes/**` placeholders | App shell, sidebar (Admin item by role), header, swimlane switcher, user menu, lazy command palette, shortcuts sheet, toaster (own component, no `sonner`), theme switch (cookie), live pill, offline and paused banners, error boundary, 404 and 403 and error screens, `EmptyState`, `ErrorState`, `PermissionState`; the keyboard registry with `g then` sequences (1.2 s timeout) and the rule that single keys are off while an input, editor or dialog has focus | none |
| S1 access | `src/features/access/**` | Sign in, access pending, disabled, onboarding (states idle, verifying, ok, classic, repo, mismatch from the problem codes in F6), personal MCP tokens | C7 for typed codes |
| S2 swimlanes | `src/features/swimlanes/**` | Overview (live), detail tabs services, drift, findings, activity; services list with restart dialog | C3 |
| S3 file view | `src/features/file/{view,diff,edit,history,dialogs,pr,docs-panel}/**` (one directory each; `FilePage.tsx` is the S3 integrator's) | Tabs NFS, Git, Baseline; header facts and line-ending flag; binary side-by-side from `/api/v1/blobs/{hash}`; ref picker; Monaco edit wiring (expected hash, `Idempotency-Key`, line ending kept, validation, secret block, save bar); 409 re-apply, 423 reason, `proposal_created` "sent for approval" with link; history and revert; upload, delete and effect preview; PR dialog with the explicit base or tenant choice and repo paths | C3, C4, C7 |
| S4 compare | `src/features/compare/**` | Grid (channel filter, differing rows by default, cursor pages) and whole-swimlane compare (summary, differing files, per-file diff) | C4 |
| S5 search, docs, status | `src/features/{search,docs,status}/**` | Text (literal or regex, glob, jump to line), settings, docs browse, search, reader, upload and replace dialogs, feature status | C5 for the reader |
| S6 findings, adoption | `src/features/{findings,adoption}/**` | Findings jumping to lines; adoption wizard | C2 |
| S7 approvals, audit | `src/features/{approvals,audit}/**` | Queue, drawer, decide with 409; audit with filters and cursor | C6, C8 |
| S8 admin | `src/features/admin/**` (one file per section: users and access requests, swimlanes and agents with tenants and auto-notify, paths, merge, tiers, folders, notify, MCP) | Generic CRUD table kit, role and status edits, join token | none |

- **Phase C and shared.** `e2e/visual/{matrix.ts,thresholds.json}`, `e2e/keyboard/*`, `e2e/journeys/*`, `e2e/a11y/*` and `design/DEVIATIONS.md` are the implementer's; each slice adds its own `e2e/visual/<slice>.spec.ts` and `e2e/a11y/<slice>.spec.ts`.
- **Graphify, per slice.**
  - S1: the problem kinds in `guard/problem.rs` and `routes/auth.rs`; `Me`, `GithubCredential`; the 422 codes in `hub-writes/src/routes/me.rs`.
  - S2 and S6: `SwimlaneSummary`, `SwimlaneDetail`, `ServiceState`; `Finding`, `SettingLocation`; whether `05` registers any adoption route.
  - S3: `WriteResult`, `ConflictProblem`, `LockedProblem`, `ChangePreview`, `PreviewWarning`, `PrTarget`, `CreatePrRequest`, `PrJob`; `FileEntry`, `FileContent`, `Comparison`; the `duplicate_pr` problem in `routes/problem.rs`.
  - S4 and S5: `GridQuery`, `TreeComparison`, `TextQuery`, `TextResults`, `DocText`, `DocHit`, `FlagStatus`; the caps (`max_total`, `max_per_file`, 500 per page).
  - S7 and S8: `Proposal`, `AuditEvent`, `User`, the `admin*` operation schemas, and where `x-required-role` is enforced so role gates match.

### T5: Attribution, severity, PR links, masking and sentinels

- Attribution chip (source, confidence, actor) on history entries, the activity feed, audit rows and file headers, from `Version.attribution`, `FileEntry.attribution`, `AuditEvent.attribution`. Severity badges with sort and filter from `severity` fields and `min_severity`. "PR open" and "fix merged, awaiting sync" badges from `listPrs` joined on swimlane and path. Duplicate-PR feedback from the 409 problem (needs C7 for `link`). PR job progress view driven by `pr_job_progress` and `getPrJob`; never fakes completion.
- Masking: flagged values arrive as `[REDACTED: N chars, rule]` and render as a mask with an audited Reveal (Editor and above, reason required, "recorded in the audit log"). The revealed value lives only in component state: the reveal call uses `gcTime: 0`, is not keyed into a persistent cache, and is dropped on navigation. Denied files show "content withheld" and no edit actions.
- Admin: sentinel status, OS Login mapping, severity rules, retention with `dry_run` preview.
- **Fan-out.** A attribution and severity components and their list integration; B PR badges, duplicate-PR feedback, job progress view; C masking, reveal, denied files, and the DOM test; D the four admin sections. Each adds new files; the implementer makes the one-line wiring edits in the T4 pages (`FilePage.tsx`, history, audit rows, admin section registry).
- **Graphify.** (1) `Attribution`, `Evidence`, `Actor`, `Confidence` and `AttributionSource`. (2) `domain::secret` and the `[REDACTED: N chars, rule]` format. (3) `PrLink`, `PrJob`, `PrJobState` and the PR job publishers. (4) The admin sentinel, OS Login, severity and retention schemas.

### T6: Pickup states, notify and "As served"

- Pickup chips replace any "pending restart" wording: Live; "Live by HH:MM (client cache)" from `live_by`; Needs notify or restart; Needs config-server restart. Chips come from `ServiceState.pickup` and `FileEntry.pickup_state` and show `notifications_enabled` as reported (no assumption; Q35 only changes copy).
- "Notify services" after a write (Operator) calls `notifyServices` and reports `accepted`; per-swimlane auto-notify in admin settings; "Restart config-server" when finding C11 is open (needs the config-server `ServiceRef`, C3).
- "As served" tab: application, tenant and channel pickers call `getServedFile`, and compare the answer with the rendered view and with NFS through `compareFile`.
- **Fan-out.** A pickup chip and its integration in overview, services, tree and the save result panel; B notify, auto-notify and restart-config-server actions; C the As served tab. Wiring edits by the implementer.
- **Graphify.** (1) `PickupState` and `ServiceState` semantics. (2) `NotifyRequest`, `NotifyResult` and the audit behaviour of `notifyServices`. (3) `ServedFile.sources`. (4) `FindingKind::ConfigServerRestartRequired` and how C11 is raised.

### Existing scaffold

| File | Action |
|---|---|
| `web/package.json` (React 19.3.0, Vite 6.4.3, Vitest 3.2.7, ESLint 9, TypeScript 5.8.3, pnpm 10.34.6) | **Extend**: new scripts (`gen:api`, `test:e2e`, `build` ending in the bundle gate), new dependencies below, `engines.node` to `>=22.18` (orval). Existing caret ranges stay; new dependencies are pinned exactly. |
| `web/pnpm-lock.yaml`, `web/pnpm-workspace.yaml` (`minimumReleaseAge: 20160`), `web/.nvmrc` | **Keep**; the lockfile is regenerated. Install scripts stay blocked; any package needing one is named in the PR. |
| `web/tsconfig.json` (strict, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`) | **Extend**: `@` alias, a second config for `e2e` and `scripts`. Strictness unchanged and guarded by a test. |
| `web/eslint.config.js` (`react/no-danger`, `no-eval`) | **Extend** with the T1 rules. The "docs renderer exception" comment is made real by the single file override. |
| `web/vite.config.ts` | **Extend** (T1 and T2 list). |
| `web/index.html` | **Replace** (no nonce placeholder today). |
| `web/src/{App.tsx,App.test.tsx,main.tsx}`, `web/src/styles/tokens.css` | **Replace** (placeholders; their own comments say prompt 08 replaces them). |
| `web/src/` everything else | **New.** |

## Cross-cutting designs

### How the CSP test gets the production policy

- `crates/hub/examples/web_fixture.rs` builds a `hub::Hub` with `Deps { static_files: Some(StaticFiles::open(web/dist)), .. }` and `HubSettings::from_lookup(env)`, then serves `Hub::web` on loopback. The CSP header, the nonce substitution, the `trusted-types` list, `Cache-Control` and HSTS are the production code path (`security_headers`, `html_csp`, `StaticFiles`), not a copy. `LK_TRUSTED_TYPES` is read from the environment exactly as in a deployment (default: the two names).
- `web/e2e/harness/serve.mjs` is a Node server on one origin. It proxies every non-API path to the fixture untouched (status, headers and body pass through) and answers `/api/v1/**` and `/auth/**` from the orval MSW handlers using `msw`'s `getResponse` and `sse` (both exported by `msw@3.0.1`, checked in the tarball), with scenario switching and event emission through a control endpoint. A service worker is not used: registering one with a plain string URL is governed by Trusted Types in Chromium, and the policy has no name to spare (the spike confirms).
- `e2e/csp/csp.spec.ts` visits every route in `routes.generated.ts` and opens every overlay (dialogs, alert dialogs, popovers, palette, menus, selects, toasts, tooltips, Monaco), in light and dark. It records `securitypolicyviolation` events from an init script plus console errors and page errors, and fails on any. It also asserts the served header contains `require-trusted-types-for 'script'` and no `unsafe-` token, so a relaxed fixture cannot make the test vacuous. `assets.spec.ts` requests every file in `dist` and expects 200 with the right type (this catches the hub's filename rules without copying them). `requests.spec.ts` fails on any request to another origin.

### Monaco under S12

1. **Gate spike (T1, first).** Run the lazy editor chunk under the fixture's CSP and Trusted Types with the editor features that need a worker or HTML switched off (`links`, `wordBasedSuggestions`, `quickSuggestions`, hover, `occurrencesHighlight`, `colorDecorators`, unicode highlighting), languages limited to YAML, JSON, XML, properties and plain text from `basic-languages` (no `editor.main`, no JSON or TypeScript language workers).
2. **One Trusted Types policy.** `MonacoEnvironment.createTrustedTypesPolicy` returns the single `lanekeeper-monaco` policy for every name Monaco asks for. Its `createHTML` is a pass-through (Monaco escapes model text when it builds line HTML, as in VS Code on the web); its `createScriptURL` accepts only same-origin `/assets/*.js` URLs. No `default` policy is ever created (it would hide violations).
3. **Workers.** `getWorker` loads `editor.worker` as a module worker from its hashed asset URL (`?worker&url`, never Vite's `?worker` wrapper, which passes a plain string, and never `?worker&inline`, which needs `blob:`), wrapped by `createScriptURL`. The worker's script response carries the API CSP (`default-src 'none'`), which allows a self-contained worker; the spike confirms it.
4. **Runtime `<style>`.** Monaco creates `<style>` elements with no nonce. A shim installed with the editor chunk wraps `Document.prototype.createElement` so a `style` element gets `.nonce` from the page nonce before it is appended. This uses the CSP's own nonce mechanism and adds no `unsafe-inline`; markup injected by an attacker does not go through the app's `createElement`.
5. **Direct DOMPurify policy.** Hover, suggestions and markdown rendering are off, so the vendored DOMPurify should never run; the spike asserts zero `trusted-types` violation reports. If it does run, the fix is a `TRUSTED_TYPES_POLICY` config, not a new policy name.
6. **Stop rule.** If the spike needs `unsafe-inline`, `unsafe-eval`, `blob:` or more than the two allowed names, stop and escalate (prompt rule). The alternatives are for a human: widen `LK_TRUSTED_TYPES` (a hub config value, not a relaxation of the directive set) or replace Monaco (a change to D31).
7. **Other injectors.** `sonner` and Radix `ScrollArea` are not used (both inject a runtime `<style>`; the handoff flags both): a small toaster and native scrolling with styled scrollbars, as the handoff suggests. `cmdk`, `react-resizable-panels` and the Radix overlays are covered by the CSP spec's overlay sweep.

### Query keys and SSE invalidation (T2 keys, T3 plan)

Keys (all in `src/lib/query/keys.ts`; `immutable` ones are hash-keyed and are never invalidated or refetched on resync):

`['me']`, `['swimlanes']`, `['swimlane', id]`, `['pickup', id]`, `['tree', id, prefix, drift]`, `['file-head', id, path]`, immutable `['content', id, path, hash]`, `['content-ref', id, path, repo, ref]`, immutable `['compare', left, leftHash, right, rightHash, file, channel]`, `['history', id, path]`, `['grid', file, lanes, onlyDiff, channel]`, `['compare-tree', left, right, prefix]`, `['findings', params]`, `['proposals', params]`, `['proposal', id]`, `['prs', params]`, `['pr-job', id]`, `['docs', params]`, `['doc', id]`, `['features', params]`, `['audit', params]`, `['admin', section, params]`.

| Event (wire `type`) | Fields | Invalidates (active queries refetch, inactive are marked stale) |
|---|---|---|
| `file_observed` | swimlane, path, hash, source | `['file-head', swimlane, path]` unless the cached head hash equals the event hash; tree pages of `swimlane` whose prefix is an ancestor of `path`; `['history', swimlane, path]`; `['swimlane', swimlane]`; grids whose lanes include `swimlane`. Toast for `source: scan` (action "View swimlane") |
| `drift_changed` | swimlane, paths | `['swimlanes']`, `['swimlane', swimlane]`; tree pages and file heads for the paths (above 50 paths: all `['tree', swimlane]`); `['compare-tree']` for the lane |
| `findings_changed` | swimlane | `['findings']` (lane or all), `['swimlane', swimlane]` |
| `pickup_changed` and `pending_restart_changed` | swimlane, services | `['pickup', swimlane]`, `['swimlane', swimlane]` (one handler for both) |
| `proposal_changed` | id, status | `['proposals']`, `['proposal', id]`; toast when the proposal is the user's own (cached or fetched, because the event carries no author) |
| `agent_status_changed` | swimlane, status | `['swimlanes']`, `['swimlane', swimlane]`, `['admin', 'agents']` |
| `git_head_moved` | repo, branch, from, to | `['swimlanes']`; `['content-ref']` entries whose `ref` is the branch |
| `access_request_created` | user | `['admin', 'access-requests']` and the sidebar badge (Admin only; the server filters) |
| `doc_indexed` | doc | `['docs']`, `['doc', doc]`, `['features']` |
| `attribution_updated` | swimlane, path, hash | `['history', swimlane, path]`, the covering tree pages, `['file-head', swimlane, path]` |
| `pr_state_changed` | link_id, state | `['prs']` |
| `pr_job_progress` | job_id, state | `['pr-job', job_id]`; `['prs']` on a terminal state |
| `sync_window_opened` and `sync_window_closed` | swimlane, job | `['swimlane', swimlane]`, `['swimlanes']` |
| `resync` | none | all active non-immutable queries |
| `user_sessions_revoked` | none | never arrives (dropped at the edge); a 401 is handled by the API client |

What the UI needs that the contract lacks (also in C1 to C8): no `DomainEvent` payload schema; no audit event (so audit lists refetch on mount and through the "n updates" chip fed by `file_observed`, `proposal_changed`, `pr_state_changed`, `access_request_created`; no polling); no baseline or adoption event (admin screens refetch on mount); `proposal_changed` has no author or swimlane; `pr_state_changed` and `pr_job_progress` have no swimlane or path; no event for a swimlane's tenants or auto-notify changing.

### P10 bundle arithmetic (estimates; T1 measures a shell build as the first checkpoint, T2 gates)

| Part (gzipped KB, estimate) | KB |
|---|---|
| react and react-dom 19 | 58 |
| TanStack Router | 14 |
| TanStack Query | 13 |
| TanStack Virtual | 4 |
| Radix primitives used by the shell (dialog, tooltip, dropdown, popover, tabs, toggle group, select, with floating-ui and scroll lock) | 32 |
| lucide icons used by the shell and the landing route (about 40) | 14 |
| tailwind-merge, cva, clsx | 9 |
| API client: mutator, ETag cache, problem parser, about 8 operations | 6 |
| App: shell, router table, keys, SSE client and invalidation, keyboard registry, toaster, state kits | 35 |
| **Total** | **about 185 of 250 (headroom 65)** |

Not in the initial closure by construction (checked by the gate): Monaco (about 450 KB gzipped for the subset; no contract budget, reported only), `cmdk` (loaded on first palette open, with an idle prefetch), docs HTML sink, the adoption wizard, admin, and every route chunk, plus `msw` and `faker` (dev and test only, imported behind `import.meta.env.MODE`). Cut order if the number passes 225: palette and shortcuts sheet to lazy, then icons to route chunks, then Radix select and popover to the routes that use them.

### Visual regression: references, thresholds, tiers

- **References are rendered, not exported.** There are no exported PNGs in the handoff, only `.dc.html`. `e2e/visual/render-references.mjs` starts a throwaway static server over `design/handoff/screens` (read-only), opens each page in a sandboxed Playwright context with no CSP and none of the app's code, serves the three unpkg URLs `support.js` asks for from `node_modules` (`react18`, `react-dom18`, `@babel/standalone`, same versions), sets `?theme=dark` for dark, rewrites the `data-props` defaults in memory to select a state, freezes the clock and disables animations, and screenshots the page root. Output goes to `web/e2e/visual/.refs/` (gitignored) with a committed `manifest.json` (sha256 of each source file, `support.js`, the render options, the Playwright version). Because reference and app are rendered by the same Chromium and fonts, anti-aliasing differences between machines do not enter the comparison. The renderer is a dev tool; nothing from the handoff is imported by the app.
- **Tiers** (the matrix is generated from `states.generated.ts` and the page files' `data-props`, so it cannot drift):
  - Tier 1, pixel comparison against a full-size reference: the nine `Shell` pages at 1440x900 in both themes (18 shots); the 17 "wired" States rows (loading 6, empty 5, error 2, truncated 1, offline 2, permission 1) as the page file's 1216x856 region, which the app screenshot takes as its `[data-region="page"]` element at a 1440x900 viewport (34 shots); the `Access Screen` states at their 1280x800 preview (the prompt's 1440x900 does not apply to this one, recorded in `DEVIATIONS.md`).
  - Tier 2, for the 50 specimen-only rows: the reference's visible text and roles are extracted from the rendered frame, and the app's state region must contain them (title, explanation, request id control, buttons, the disabled-with-tooltip reason). The app also gets its own committed baselines (`e2e/visual/__baselines__/`) that guard regressions from then on; these are not "against the design" and the PR says so.
- **Thresholds.** Playwright `toHaveScreenshot` with `threshold: 0.2` per pixel, `animations: 'disabled'`, `caret: 'hide'`, clock frozen, `maxDiffPixelRatio` ceiling of 0.5% for tier 1. The first full run is reported (ratio per screenshot) in `plans/08-frontend.evidence.md`; per-screen overrides go in `e2e/visual/thresholds.json` with a reason and are valid only with the design owner's approval. No threshold above 0.5% is merged without it.

### Keyboard-map tests

`extract-keymap.mjs` reads the `keys` table of the Engineering Handoff page and expands it into rows (`g then s / c / f / i / a` into five, `⌘1 … ⌘9` into nine, `Space or x` into two, `↑ ↓`, `→ / ←`), with `⌘` normalised to `ControlOrMeta` (the app accepts Ctrl on non-Mac). `e2e/keyboard/bindings.ts` is a hand-written executable binding per row id. `keymap.spec.ts`:

1. generates one keyboard-only test per row (a fixture fails the test if any pointer event reaches the page);
2. checks completeness both ways: every extracted row has a binding, and every shortcut in `src/keyboard/registry.ts` is in the handoff table;
3. runs the negative cases (single keys are off in an input, the editor and dialogs; sequences time out after 1.2 s on a fake clock; Enter never confirms a `DestructiveConfirm`).

The Style Guide's conflicting sheet is not tested; the handoff says this page follows Shell, and the design owner is asked to fix the other (W13).

### `design/DEVIATIONS.md` candidates (rows are written when built; each needs the design owner's approval)

| Area | Proposed deviation | Reason |
|---|---|---|
| Theme | `/theme-init.js` plus `lk_theme` cookie instead of the server writing `class="dark"` (the hub only substitutes the nonce). The handoff names this as its own alternative | security (CSP) |
| Toasts, scroll areas | Own toaster and native scrolling instead of `sonner` and Radix ScrollArea | security (CSP, Trusted Types) |
| Icons | Inline `lucide-react` components instead of CSS-mask files | security and performance |
| Read-only file view and diff | `CodeView` and `DiffView`, not Monaco; an outline rail instead of a minimap | performance (P10 diff in 500 ms) |
| Monaco chrome | Find widget, context menu and suggest widget keep Monaco's own look, themed from the tokens | security (no inline style, no extra styling hooks) |
| Audit visibility | Admin only, as the contract says, where the handoff says Viewer and above | contract and security |
| Live reconnect | Replay by `Last-Event-ID`; refetch only after Offline or on `resync` | performance (P13, no refetch storm on every 30 minute rotation) |
| Search truncation copy | Uses the server's `truncated` flag and cap (500) where the handoff says 1,000 | contract |
| Diff "modified" line kind | Removed plus added pair shown as changed, no `~` kind (not in `DiffLineKind`) | contract |
| Access screens viewport | Compared at 1280x800 | the reference's own preview size |
| Approvals filter | Tabs `pending, stale, expired, decided` instead of `withdrawn` | contract |

### New dependencies (each justified; exact pins; release date at least 14 days before 2026-10-16, re-checked at T0 with the same script)

| Package and version (release date) | Kind | Why |
|---|---|---|
| `@tanstack/react-router@1.170.41` (09-30), `@tanstack/react-query@5.104.0` (09-26), `@tanstack/react-virtual@3.14.13` (09-14) | runtime | The fixed stack (D31): URL state, cache, virtualisation. Code-based routes, so no router plugin |
| `radix-ui@1.6.7` (07-24), `class-variance-authority@0.7.1`, `clsx@2.1.1`, `tailwind-merge@3.7.0` (09-12) | runtime | shadcn/ui primitives and variant utilities |
| `lucide-react@1.49.0` (09-29) | runtime | Icons as inline SVG (the handoff's CSP review) |
| `cmdk@1.1.1`, `react-resizable-panels@4.14.1` (09-27) | runtime, lazy where possible | Palette; split pane |
| `monaco-editor@0.57.0` (09-24) | runtime, lazy chunk only | The editor (fixed stack). Brings `marked@14.0.0` and `dompurify@3.4.15` into that chunk |
| `tailwindcss@4.3.3`, `@tailwindcss/vite@4.3.3`, `tw-animate-css@1.4.0` | build | Tailwind (fixed stack), v4 CSS-first config |
| `orval@8.39.0` (09-30), `msw@3.0.1` (09-30), `@faker-js/faker@10.6.0` (08-14) | build and test | Client and mocks (fixed stack); `getResponse` and `sse` drive the e2e harness |
| `@testing-library/react@16.3.3`, `@testing-library/user-event@14.6.7`, `@testing-library/jest-dom@7.0.1`, `jsdom@30.1.1`, `axe-core@4.13.0` | test | Component tests and component-level accessibility |
| `@playwright/test@1.63.0` (09-04), `@axe-core/playwright@4.13.0`, `@lhci/cli@0.15.1` (fallback `lighthouse@13.5.0`, W19) | test | E2E, visual, keyboard, axe, Lighthouse CI (fixed stack) |
| `eslint-plugin-jsx-a11y@6.10.2`, `eslint-plugin-react-hooks@7.1.1` | lint | Accessibility and hooks rules (S21 equivalents) |
| `acorn@8.18.0` (07-28) | build | Reads the handoff pages' data statically; never executes them |
| `react18` as `npm:react@18.3.1`, `react-dom18` as `npm:react-dom@18.3.1`, `@babel/standalone@7.29.0` (2026-01-31) | dev only, never bundled | The exact files `support.js` fetches from `unpkg.com`; they let the reference renderer run offline in a sandbox (W15) |

Not adopted: `sonner`, `@radix-ui/react-scroll-area`, `@monaco-editor/react` (its loader pulls Monaco from a CDN), TanStack Table, `@tanstack/router-plugin`, `zod`, direct DOMPurify, `react-hook-form`. The Playwright and Lighthouse browser binaries are tooling like a Rust toolchain, not dependencies; they are installed in CI and on developer machines (W20).

## Traceability

| Requirement (acceptance item, S#, P#) | Task | Proof (test, bench or fuzz target name) |
|---|---|---|
| Objective: stays inside a strict CSP | T1 | `e2e/csp/csp.spec.ts`, `e2e/csp/requests.spec.ts`, `e2e/csp/assets.spec.ts` |
| Objective: updates live over SSE | T3 | `live/sse-client.test.ts`, `live/invalidation.test.ts`, `e2e/live/live.spec.ts` |
| Objective: responsive on 6,000-line diffs and 5,000-row grids | T2 | `e2e/perf/diff-6000.spec.ts`, `e2e/perf/grid-5000.spec.ts` |
| Objective: no AI chat | T4 | `routes/inventory.test.ts` (the router's table equals the handoff inventory; no other route or nav item) |
| Acceptance 1: `just verify-08` and every P10 gate pass | T1, T2 | the `verify-08` recipe; the five P10 proofs below; `cargo xtask bench-check` on the five ids |
| Acceptance 2: CSP violation test passes | T1 | `e2e/csp/csp.spec.ts` (production header from `web_fixture`), `e2e/csp/monaco.spec.ts` |
| Acceptance 3: no API type written by hand | T1 | `api-types.guard.test.ts` (no type, interface or enum outside `src/api/generated/` has a name equal to a `components.schemas` key); `gen:api` freshness check; ESLint ban on `fetch('/api` outside the mutator |
| Acceptance 4: visual and keyboard parity; deviations listed | T4 | `e2e/visual/*.spec.ts`, `e2e/keyboard/keymap.spec.ts`, `deviations.test.ts` (every threshold override and every `test.fixme` has a row in `DEVIATIONS.md`; the PR lists rows still unapproved) |
| S2 sessions (client side) | T1 | `auth/session.test.tsx` (401 clears the cache and goes to sign-in with a same-origin `return_to`; 403 pending and disabled screens), `lint/rules.test.ts` (no web storage), `lib/query/no-persist.test.ts`, `auth/logout.test.tsx` (POST with CSRF) |
| S3 CSRF | T1 | `api/client.csrf.test.ts` (iterates every non-GET operation in `api/openapi.yaml`: `X-CSRF-Token` and `Idempotency-Key` present, GET carries neither), `api/constants.drift.test.ts` (reads `CSRF_HEADER` and `CSRF_COOKIE` from the Rust source), `lint/rules.test.ts` (no `<form method="post">`) |
| S12 output safety | T1, T2 | `e2e/csp/csp.spec.ts`; `lib/csp/trusted-types.test.ts` (exactly the two names, each created once); `lint/rules.test.ts`; `components/app/SanitisedDocHtml.test.tsx` (XSS corpus through the one sink); `dist.test.ts` |
| S21 lint equivalents | T1 | `lint/rules.test.ts`; `tsconfig.test.ts` (strict flags); `ErrorState.test.tsx` (title, request id, and the server's `detail` only for the known codes of F6) |
| P10 initial bundle under 250 KB gzipped | T2 | `scripts/perf/check-bundle.mjs` (in `pnpm build`), result `web/initial_js_gzip_kb` |
| P10 route change under 100 ms cached | T2 | `e2e/perf/route-change.spec.ts`, result `web/route_change_cached` |
| P10 6,000-line diff interactive in 500 ms | T2 | `e2e/perf/diff-6000.spec.ts`, result `web/diff_6000_lines_interactive` |
| P10 5,000-row grid at 60 fps | T2 | `e2e/perf/grid-5000.spec.ts`, result `web/grid_5000_rows_scroll_fps` |
| P10 Lighthouse performance at least 90 | T2 | `lighthouserc.json` run, result `web/lighthouse_performance` |
| P13 live update within 1 s | T3 | `live/sse-client.test.ts` (event to invalidation call within one coalescing window, fake timers), `e2e/live/live.spec.ts` (emit to DOM under 1 s, written to `target/perf-results/web-sse-to-dom.json`, informational); the server side stays `P13` (`hub-platform/sse_delivery`) |
| T0 Verify: token parity | T0 | `tokens.parity.test.ts` |
| T1 Verify: lint ban works | T1 | `lint/rules.test.ts` |
| T3 Verify: mocked event stream | T3 | `live/sse-client.test.ts`, `live/sse-parser.prop.test.ts` |
| T4 component tests | T4 | `features/file/pr/PrDialog.test.tsx`, `features/file/edit/conflict-flow.test.tsx` (409 shows what changed and re-applies), `features/file/edit/approval-sent.test.tsx` (200 `proposal_created`), `features/file/edit/locked.test.tsx` (423), `features/file/dialogs/EffectPreview.test.tsx`, `features/access/Onboarding.test.tsx`, `features/admin/AccessApproval.test.tsx`, `features/adoption/Wizard.test.tsx`, `features/file/edit/crlf-save.test.tsx` (fake `EditorSurface`), `e2e/editor/crlf.spec.ts` (real Monaco end of lines) |
| T4 visual regression, light and dark, every screen and state | T4 | `e2e/visual/*.spec.ts` generated from the States index |
| T4 keyboard maps | T4 | `e2e/keyboard/keymap.spec.ts` |
| T4 end-to-end smoke | T4 | `e2e/journeys/j1.spec.ts` to `j7.spec.ts` (the seven journeys of `design/13`) |
| T4 axe | T4 | `e2e/a11y/*.spec.ts`, plus component-level `axe-core` in dialogs |
| T5 component and visual tests | T5 | `AttributionChip.test.tsx`, `SeverityBadge.test.tsx`, `PrBadge.test.tsx`, `PrJobProgress.test.tsx`, `e2e/visual/t5.spec.ts` |
| T5 masked values never reach the DOM before a reveal | T5 | `features/file/masking.dom.test.tsx` and `e2e/masking.spec.ts` (assert on the DOM and the query cache) |
| T6 component and visual tests | T6 | `PickupChip.test.tsx`, `NotifyServices.test.tsx`, `AsServed.test.tsx`, `e2e/visual/t6.spec.ts` |

## Tests, benchmarks and fuzz targets

- **Unit and component** (Vitest, jsdom, Testing Library): the files named above. Monaco does not run in jsdom; component tests use a fake `EditorSurface`, and real Monaco is covered by Playwright.
- **Property tests** (seeded generator in `src/test/prng.ts`, at least 10,000 cases, seed printed on failure): the SSE parser (any chunk split equals the unsplit parse), the URL search-parameter parsers (user-controlled input), the byte-range to UTF-16 converter on non-ASCII and emoji lines, the problem parser, the key-sequence matcher.
- **Golden tests:** the handoff extractors and the generated `src/handoff/*`, the event-to-invalidation plans (`toMatchFileSnapshot`), the shortcut table.
- **Integration:** there is no database in the web app. The equivalent is the e2e run through the hub's own header code (`web_fixture`) with MSW handlers.
- **Fuzz:** cargo-fuzz covers Rust parsers (S22) and does not apply to TypeScript. The web parsers above get the property tests instead; this is a deliberate substitution, stated here.
- **Benchmarks:** the five P10 results are written to `target/perf-results/` and checked by `cargo xtask bench-check` after the `registered` flip (C9). No new budget entries.

## Extra paths and contract changes

### Extra-paths

| Path | Reason |
|---|---|
| `crates/hub/examples/web_fixture.rs` | New file, no change to existing hub code and no new dependency (the hub's dev-dependencies already carry the fakes). It is the only way to run the CSP, perf and visual tests under the production header code instead of a copy (F2, F4). Rust implementer. |
| `.github/workflows/web-e2e.yml` | New workflow. The existing `ci.yml` `web` job has no browser. This job installs Chromium, builds the fixture, and runs Playwright, the P10 specs and Lighthouse. Actions pinned by SHA (S19). The human may fold it into `ci.yml` (prompt 09's file) instead. |
| `docs/threat-model.md` | A "Web app (08)" section in STRIDE form (S24; `lanekeeper-security` step 4): XSS and Trusted Types sinks, CSRF and the `Origin: null` form quirk, session handling in the browser, supply chain of the build, masked values, SSE abuse. |

### Contract-change: needed

One additive `contract:` PR before `08/T3`, for the human to approve. Nothing is removed or renamed. Rejected as workarounds: a web-side OpenAPI overlay and hand-typed unions (both break acceptance 3).

| Id | Change | Why | Blocks | If declined |
|---|---|---|---|---|
| C1 | `DomainEvent` schema (one object per variant, discriminated by `type`) in `components.schemas`, linked from the SSE response with `x-event-schema`; document `InlineRange` and `TextHit.ranges` as UTF-8 byte offsets | The SSE body is `type: string`, so event types cannot be generated | T3 | T3 cannot start |
| C2 | Operations for the adoption wizard (scan state with branch ranking and hints, per-file resolutions with bulk, confirm which sets baselines and unlocks editing), findings actions (mark and clear intentional divergence, restore a lost version for C7) | None exist (grep for `adopt` and `intentional` finds enums only) | T4 S6, journey 6 | S6 is not built |
| C3 | `SwimlaneSummary`: `tier`, `baseline` (state, who, when), `pending_services` (count), `syncing`; `SwimlaneDetail.config_server` as `ServiceRef` or null; `LockedProblem.reason` (`sync_window`, `pr_job`, `baseline_unconfirmed`) and `holder` | The overview must show tier, baseline state and pending restarts live without 40 calls to `listPendingRestarts`; the lock reason is required by T4 and the States index; "Restart config-server" needs a target | T4 S2, S3; T6 | Those parts show only what exists |
| C4 | `GET /api/v1/git/refs?repo=base or tenant` (branch, tag and version names) | Compare targets "branch" and "tag" and the file ref picker have no source | T4 S3, S4 | Free-text refs only |
| C5 | A sanitised rendered form of a doc (`html` and `headings`), on `readDoc` or as `GET /api/v1/docs/{id}/rendered` | S12 says the server renders and sanitises Markdown, but no operation returns HTML | T4 S5 | The reader shows the Markdown source as plain text through no HTML sink; `SanitisedDocHtml` waits |
| C6 | `GET /api/v1/proposals/{id}` | The deep link `/approvals/:id` cannot reach an old proposal through the list | T4 S7 | Deep links work only for proposals on loaded pages |
| C7 | `Problem.code` and a documented `ProblemCode` list (F6); the duplicate PR conflict with `reason: duplicate_pr` and `link` | The UI branches on these and the generated types lack them | T4 S1, S3; T5 | Branch on the `type` URN string without generated types (weaker) |
| C8 | A viewer-scoped audit read | Only if the human rejects W6 (Audit is Admin-only as `adminListAudit` says) | T4 S7 | Default W6 applies |
| C9 | `perf/budgets.toml`: `registered = true` on `P10.initial_js_gzip_kb`, `P10.route_change_ms`, `P10.diff_6000_lines_ms`, `P10.grid_5000_rows_fps`, `P10.lighthouse_perf` (the file's own comment says the owning prompt flips them). No threshold changes | The gate needs them registered | T2 | The P10 gates would report UNMET |

## Ambiguities and open questions

| Question | Where it came from | Proposed default (configurable) | Needs a human? |
|---|---|---|---|
| W1 Approve the contract PR (C1 to C9), and who writes C2 (prompt 05 or 06 owners) | F10 | One additive `contract:` PR before T3; C2 shapes written by the registry owners | Yes |
| W2 Browser gates cannot run in the agent sandbox (F9) | Environment | They run in `web-e2e.yml` and on developer machines; `just verify-08` fails with install instructions when Chromium is missing and never skips; the evidence file says which gate ran where | Yes (accept CI as the evidence source, or provide a browser mirror) |
| W3 `design/handoff/README.md` is not at that path | Task note | Use `design/handoff/screens/README.md` | No |
| W4 Visual references: 50 of 67 States rows are 600x380 specimens | F8 | The tier model above; or the design owner supplies full-size renders in `design: handoff v2` | Yes (design owner) |
| W5 Pixel thresholds | Prompt T4 | 0.2 per pixel, 0.5% ceiling, overrides only with design owner approval | Yes (design owner) |
| W6 Audit is Admin-only in the contract, Viewer and above in the handoff | F10 | Contract wins; Admin-only; a `DEVIATIONS.md` row | Yes (product) |
| W7 Approvals deep link and `withdrawn` | F8, C6 | Add `GET /proposals/{id}`; tabs from the page file, no `withdrawn` | Design owner to confirm |
| W8 Routes the handoff table lacks: access disabled, personal MCP tokens, and the full admin section list (the Admin page has 12). The contract has only `/me/mcp-tokens` (the caller's own tokens), so the Access screen's `mytoken` and the Admin `mcp` section are the same component | Handoff, OpenAPI | `/access/disabled`, `/me/tokens`, `/admin/{users,swimlanes,paths,merge,tiers,folders,notify,mcp,sentinels,oslogin,severity,retention}`; one `PersonalTokens` component mounted at both | Design owner to confirm |
| W9 Does the read-only file view use Monaco? | Prompt T2 versus P10 | No: custom virtualised `CodeView`; Monaco only in Edit mode; an outline rail replaces the minimap | Yes (design owner) |
| W10 Monaco gate fails | Prompt stop rule | Stop and escalate; options are to widen `LK_TRUSTED_TYPES` or to replace Monaco (changes D31) | Only if it fails |
| W11 Reconnect: handoff says refetch instead of replay | Handoff versus prompt T3 | Replay by `Last-Event-ID`; refetch after Offline or on `resync` | Design owner (deviation row) |
| W12 Theme storage | F1, F2 | `lk_theme` cookie read by `/theme-init.js`; no hub change | Design owner (deviation row) |
| W13 Two shortcut sets | F8 | Follow Shell | Design owner to fix the Style Guide sheet |
| W14 Tailwind v4 CSS-first versus the handoff's `tailwind.config` wording | Handoff | v4 `@theme inline` (what `shadcn@4.21.0` emits); every key maps to `var(--name)` | No |
| W15 Dev-only React 18, ReactDOM 18 and Babel to render references offline | F8 | Accept as test-only, never bundled, rendered in a sandbox with no app code | Yes (dependency approval) |
| W16 Reference PNGs committed or rendered at run time | T4 | Rendered at run time, manifest committed; app-owned baselines for tier 2 committed | No |
| W17 TanStack Table (handoff lists it, the fixed stack does not) | Handoff | Not adopted; own fixed-height virtual grid with sort and selection state | No |
| W18 shadcn CLI fetches component source from `ui.shadcn.com` at authoring time | `THIRD-PARTY.md` | Already pinned to `shadcn@4.21.0` there; generated source is reviewed and committed | No |
| W19 `@lhci/cli@0.15.1` has an old transitive tree that may fail `osv-scanner` | S19 | Use it if osv passes, else `lighthouse@13.5.0` with a small runner (same assertion) | Only if osv fails |
| W20 Chromium download hosts are not npm | S19 | Treat as tooling; version follows the pinned `@playwright/test`; installed only in CI and on developer machines | Yes (note for the S19 owner) |
| W21 Metric definitions (KB is 1,000 bytes; initial JS is the entry closure; fps is rAF frames per second over a 5 s programmatic scroll, minimum of 3 runs, capped at 60; route change is click to the new route's `h1` painted; diff interactive is activation to the first working `n` key with the mock answering instantly; Lighthouse is the desktop preset) | P10 | As stated; a 60 fps minimum is brittle on shared runners, so a miss is escalated with measurements and the threshold is never loosened | Yes (accept the method) |
| W22 T4 is one very large commit | AGENTS.md rule | One commit per task as required, with the slice list in the message; the human may allow `08/T4a` to `T4e` | Yes (process) |
| W23 The hub sends no `Content-Encoding` and `no-store` on `/theme-init.js` | F1, F5 | Not 08's code; recorded for the 03b and 09 owners; Lighthouse is measured as the hub serves | Yes (platform) |
| W24 Expired GitHub token has no proactive signal (`Me` carries only `needs_github_token`) | F6 | Reactive: the first `token_expired` problem sets an in-memory flag that disables Git-dependent actions until a new token is saved | No |
| W25 `200` with `outcome: proposal_created` is the "sent for approval" answer, not HTTP 202 (`createPr` returns 202 for an accepted job) | OpenAPI | Branch on `outcome`; show the link to `/approvals/:id` | No |
| W26 `Q35` (do services subscribe to notifications) blocks T6 copy | open-questions | Show `notifications_enabled` as reported; no assumption | Q35 owner |
| W27 Suppression of the Monaco chunk prefetch on slow links | P10 | Idle prefetch for Editor and above only, skipped when `saveData` is on | No |
| W28 Sort and filter by severity: server-side only where `min_severity` exists | T5 | Server filter where the API has one; sort within the loaded page elsewhere, labelled as such | No |

## Risks

- **Monaco under Trusted Types and CSP (security).** Eleven or more policy names against two allowed, a vendored DOMPurify that can fire a violation report, un-nonced `<style>` elements, workers under `worker-src 'self'` with an API CSP on the worker script. Contained by the T1 gate spike, the single-policy hook, the nonce shim, `getWorker` with `createScriptURL`, features switched off, and the stop-and-escalate rule.
- **Contract gaps block T3, T4 and T6 slices.** Without C1 to C7 the SSE client, adoption, docs reader, swimlane overview and lock reasons cannot be built to the acceptance rules. Contained by one early additive contract PR and by gating the slices instead of working around the gaps.
- **Browser gates cannot be verified in the sandbox.** CSP, visual, keyboard, perf and Lighthouse evidence comes from CI. Contained by `verify-08` failing closed, the new workflow, and the evidence file naming where each gate ran.
- **Visual-regression fidelity.** The reference runtime needs three external scripts and Babel; 50 of 67 states are specimens. Contained by the sandboxed offline renderer, the tier model, the generated matrix, and the design owner's approval of every threshold and deviation.
- **P10 headroom and measurement.** About 65 KB of estimated headroom, an uncompressed hub, a brittle 60 fps minimum on shared runners, and a 6,000-line diff payload that parses on the main thread. Contained by an early measured checkpoint at T1, a warning at 225 KB, Monaco only in Edit mode, hash-keyed immutable queries, fixed-height virtualisation, and escalation with measurements rather than loosening a budget.
- **Supply chain.** About 35 new direct dependencies, a dev-only React 18, Babel and Lighthouse trees, and browser binaries from a CDN. Contained by exact pins at least 14 days old, `minimumReleaseAge`, blocked install scripts, `osv-scanner`, and every package justified above.
- **Client-side secrets and masked values.** Reveal results, CSRF values and masked fields must not reach caches, storage, URLs or logs. Contained by no persister, no web storage outside one preferences wrapper, `gcTime: 0` on reveal, the masking DOM test, and `ErrorState` showing the title, the request id and `detail` only for known codes.
- **SSE limits.** Five streams per user means a sixth tab gets 429; streams rotate every 30 minutes; a role change applies at reconnect. Contained by backoff on 429 with a clear message, replay-only rotation, and the 401 and 403 routing tests.
