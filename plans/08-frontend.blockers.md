# G0: Monaco under the production CSP (W3)

Preflight G0 of `plans/08-frontend.md`: a measurement, not a task. No product code was written, nothing under `web/` was touched, and the harness lives outside the repository (scratch directory, listed in "Reproduce"). Date 2026-10-16. Browser: Chromium 141.0.7390.37 only (headless, driven by `playwright-core`); no other engine was available.

## Summary (numbers only)

1. **Plain Monaco (variant 1) does not render at all** under the exact `html_csp` string: 0 lines, 0 line numbers, no minimap, typing is not possible. 23 reports at load (10 Trusted Types policy refusals, 3 `style-src-elem`, 9 `innerHTML` sinks, 1 `Worker` sink), 53 to 56 page errors over a whole run.
2. **One policy plus a nonce shim plus a worker through the policy (variant 2) removes every Trusted Types and `style-src-elem` report, and leaves only `style-src-attr`**: 171 at load, 373 for a diff editor, 4,661 to 4,709 after typing, scrolling and a 4,000-line sweep. The editor renders and accepts typing, but overlay boxes lose their size (selection, current line, indent guides, unicode highlight, diff colours) and lines inserted after a scroll lose their `top` (1 of 24 scroll positions correct, worst error 68,400 px).
3. **Option (e) as the plan describes it (variant 3) runs with ZERO reports** in the editor, the side-by-side DiffEditor and all five languages, with the plan's feature set both on and off, through typing, select all, a 24-position scroll sweep of a 4,000-line document, a typing burst, find, the F1 palette, context menu, go to line, folding, wrapping, mouse selection, multi-cursor and screen-reader mode. Every rendering number equals the expected value. No CSP directive was weakened.
4. **Variant 3 is not zero in three places**: (i) DiffEditor inline mode: 3 `style-src-attr` reports from `marginElement.setAttribute('style', ...)` at `diffEditorViewZones.js:199`; (ii) any Monaco feature that renders markdown (hover, suggest documentation): 3 and 2 reports (the vendored DOMPurify asks for a policy named `dompurify`, then `DOMParser.parseFromString` and `innerHTML` are refused) and the text shown is `null`; (iii) the regex rewrite changes what the editor displays for text that contains `style="..."`: 4 of 5 test lines showed `data-lk-style=` instead of `style=`.
5. A sound shape of (e), **variant 3d** (tag-aware rewrite, MutationObserver pass, `setAttribute('style')` routed to CSSOM, one policy, nonce shim, worker through the policy), has 0 reports in everything of item 3, in DiffEditor inline mode, and 0 changed text. Markdown-rendering features remain failing (item 4 ii).
6. Option (a) measured for comparison: variant 2 served with the exact CSP plus `style-src-attr 'unsafe-inline'` has 0 reports and the same rendering numbers as variant 3d, and the same 3 and 2 reports on the markdown paths.
7. Worker scope: the editor worker was attached through CDP in 26 of the 31 runs (all runs of variants 2, 3 and their probes; variant 1 never creates one); 0 console, log or exception messages came from any worker, and CDP `Network.loadingFailed` with a blocked reason occurred in 0 runs. The worker script is served with the API policy (`default-src 'none'`) and runs.

### Variants against violation reports (compact; detail in T1 to T7)

Counts are `securitypolicyviolation` events. "Load" is page load plus 2.5 s; "after" adds the scripted interaction (typing, select all, scroll, find, F1 palette, 4,000-line sweep). "Extra stages" are the T5 stages (hover and suggest with a markdown provider, DiffEditor inline mode).

| Variant | Editor: load / after | DiffEditor: load | Languages: load / after | Extra stages with reports | Trusted Types names refused | Does it work |
|---|---|---|---|---|---|---|
| 1 plain | 23 / 57 to 60 | 29 to 30 | 24 / 50 | not reached | 10 (11 with a JSON model) | no: 0 lines drawn, typing impossible |
| 2 one policy + nonce shim | 171 / 4,661 to 4,709 | 373 | 177 / 470 | most stages (hover 5, suggest 120, inline diff 71, go to line 135, folding 132, wrapping 259, mouse selection 289, multi-cursor 52, screen-reader mode 140, context menu 8; find and replace 0) | 0 | partly: text, typing, find, palette; overlays and lines inserted after a scroll broken |
| 3 option (e), regex | 0 / 0 | 0 | 0 / 0 | inline diff 3, hover 3, suggest 2 | 0 | yes; displayed text wrong for 4 of 5 lines containing `style="..."` |
| 3d (e), tag-aware + `setAttribute` | 0 / 0 | 0 | 0 / 0 | hover 3, suggest 2 (3 with features off) | 0 | yes |
| (a) variant 2 + `style-src-attr 'unsafe-inline'` | 0 / 0 | 0 | 0 / 0 | hover 3, suggest 2 | 0 | yes |

## Setup

### Versions

| Item | Version | Published (npm) | Note |
|---|---|---|---|
| Node | v22.22.0 | | npm 10.9.4 |
| vite | 6.4.3 (exact) | 2026-06-01 | the `^6.4.3` of `web/package.json`; build `html.cspNonce: '__CSP_NONCE__'`, `build.assetsInlineLimit: 0`, `sourcemap: false`, `worker.format: 'iife'` |
| monaco-editor | 0.57.0 (exact) | 2026-09-24 | its dependencies: marked 14.0.0 (2024-08-07), dompurify 3.4.15 (2026-09-06) |
| rollup | 4.63.5 (override) | 2026-09-24 | the floating 4.64.7 was 1 day old |
| postcss | 8.5.28 (override) | 2026-09-03 | the floating 8.5.30 was 4 days old |
| nanoid | 3.3.19 (override) | 2026-09-10 | the floating 3.3.20 was 11 days old |
| esbuild 0.25.12, picomatch 4.0.7, source-map-js 1.2.2, tinyglobby 0.2.17, fdir 6.5.0 | | 2025-11 to 2026-09-30 | all at least 14 days old as of 2026-10-16 (checked for every installed package) |
| playwright-core | 1.56.1 | | preinstalled at `/opt/node-tools`; used with `executablePath: /opt/pw-browsers/chromium-1194/chrome-linux/chrome` |
| Chromium | 141.0.7390.37 | | Playwright build 1194 |

`npm install --save-exact --ignore-scripts vite@6.4.3 monaco-editor@0.57.0`, with `overrides` for the three packages above. Installed with `--ignore-scripts`; the only downloaded code that ran was Vite (to build) and Monaco's bundled output inside the browser.

### Bundle that the editor needs (all editor contributions, YAML, XML, `ini` (the `.properties` language), JSON tokens, plain text)

| File | Raw | gzip -9 |
|---|---|---|
| editor chunk (`app-*.js`) | 3,980,962 B | 1,022,152 B |
| editor CSS | 175,373 B | 27,032 B |
| `editor.worker-*.js` (iife, one file) | 275,493 B | 82,251 B |
| `codicon-*.ttf` | 153,228 B | 74,851 B |
| JSON mode chunk | 41,960 B | 12,174 B |
| `json.worker-*.js` (emitted, never requested with JSON features off) | 406,797 B | 119,900 B |
| yaml, xml, ini language chunks | 3,576 / 1,964 / 963 B | 1,317 / 772 / 508 B |

The hub sends no `Content-Encoding` (F1), so the figures a browser transfers are the raw ones.

### Server and headers

A small Node server (`tools/server.mjs`) serves `dist/`. For `/` it substitutes a fresh 128-bit hex nonce for every `__CSP_NONCE__` and sends:

```
Content-Security-Policy: default-src 'none'; script-src 'self' 'nonce-NONCE'; style-src 'self' 'nonce-NONCE'; img-src 'self' data:; font-src 'self'; connect-src 'self'; worker-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; require-trusted-types-for 'script'; trusted-types lanekeeper-docs lanekeeper-monaco
```

That string was produced two ways and compared byte for byte (326 bytes with the nonce `NONCE`): by a script that reads the `format!` literal out of `crates/hub/src/headers.rs` and applies Rust's line-continuation rule, and by compiling a verbatim copy of `html_csp` with `rustc` and calling it with the default `LK_TRUSTED_TYPES` names. Also sent, as `apply` does for HTML: `Cache-Control: no-store`, `Strict-Transport-Security: max-age=63072000; includeSubDomains`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Resource-Policy: same-origin` and the full `Permissions-Policy` list. For `/assets/<[A-Za-z0-9._-]{1,128}>`: the same headers but the API policy `default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'` and `Cache-Control: public, max-age=31536000, immutable` (so the worker script has `default-src 'none'`). Anything else is a 404 `application/problem+json` with the API policy. The real hub binary and `static_files.rs` were not run. Only the option (a) data point used a different CSP (the same string plus `; style-src-attr 'unsafe-inline'`, port 4175).

### How each variant is built

| Id | Definition |
|---|---|
| 1 plain | `MonacoEnvironment.getWorker` returns `new EditorWorker()` from Vite's `?worker` import (the documented Monaco and Vite recipe). No Trusted Types hook, no nonce shim. |
| 2 | `MonacoEnvironment.createTrustedTypesPolicy(name)` returns one policy, `lanekeeper-monaco`, for every name; `createHTML` is a pass-through; `createScriptURL` accepts only same-origin `/assets/<name>.js`. Set before Monaco is imported (Monaco is a dynamic import after the setup). Worker: `?worker&url` through `createScriptURL`, `new Worker(url)`. `Document.prototype.createElement` shim sets `.nonce` (read from `<meta property="csp-nonce">`) on `style` elements. |
| 3 = option (e) of W3 | 2, plus `createHTML` rewrites `style="..."` and `style='...'` to `data-lk-style="..."` with a regex, plus a `MutationObserver` on `document` (`childList`, `subtree`) that sets `el.style.cssText` from the data attribute and removes it. |
| 3b | 3, plus the same pass run synchronously after the `innerHTML` setter and `insertAdjacentHTML` (prototype patches). Probe beyond the plan. |
| 3c | 3b, plus `Element.prototype.setAttribute('style', v)` routed to `el.style.cssText`. Probe beyond the plan. |
| 3t | 3 with a tag-aware rewrite: only attributes inside real tags are rewritten, quoted values honoured, text between tags untouched. Probe beyond the plan. |
| 3d | 3t, plus the `setAttribute('style')` routing of 3c (no `innerHTML` patches). Probe beyond the plan. |
| (a) | 2 under the CSP plus `style-src-attr 'unsafe-inline'`. |

Features: `default` is Monaco's defaults; `off` is the plan's set (`links: false`, `quickSuggestions: false`, `wordBasedSuggestions: 'off'`, `suggestOnTriggerCharacters: false`, `hover.enabled: false`, `occurrencesHighlight: 'off'`, `colorDecorators: false`, and the JSON language service in tokens-only mode so it needs no JSON worker). The entry file imports every contribution of Monaco's `editor.main.js` (hover, suggest, links and the rest are bundled; "off" is by option), with only the five languages.

### What was recorded

For each run, from an init script that runs before the page: every `securitypolicyviolation` event (effective directive, blocked kind, sample, source file and line, disposition, and the stage that caused it); every `trustedTypes.createPolicy` call with its outcome (the real method is wrapped and re-throws); `error` and `unhandledrejection`; Playwright console and `pageerror`. From a raw CDP connection to the same page: every dedicated worker is attached while paused (so nothing is missed) and its `Runtime`, `Log` and `Network` events are collected; on the page `Log`, `Audits` (CSP issues) and `Network.loadingFailed` with a blocked reason. The number of CDP `ContentSecurityPolicyIssue` events equalled the number of DOM events in every run. Rendering: positions of every `.view-line` against `getTopForLineNumber(n) - scrollTop` (matched by text), distinct tops, sizes of overlay boxes, real keyboard and mouse input through Playwright, and screenshots.


## Observations by variant (exact)

### Variant 1, plain

- Trusted Types policy names attempted and refused (10, all at module evaluation, each also an uncaught `Error` in the page): `standaloneColorizer`, `editorViewLayer`, `richScreenReaderContent`, `domLineBreaksComputer`, `defaultWorkerFactory`, `diffReview`, `diffEditorWidget`, `tokenizeToString`, `editorGhostText`, `stickyScrollViewLayer`. Opening a JSON model asks for `defaultWorkerFactory` once more (11 in the languages run). Report: `trusted-types`, blocked `trusted-types-policy`, sample = the name.
- `style-src-elem`, blocked `inline`: 3 (editor) or 4 (diff) `<style>` elements without a nonce, all from `createStyleSheet` (app chunk, line 48).
- `require-trusted-types-for`, blocked `trusted-types-sink`: `Element innerHTML` with sample `<div style="top:0px;height:19px;line-hei...` (9 at load in the editor, 14 or 15 in the diff editor, more after typing and scrolling), and `Worker constructor` with sample `/assets/editor.worker-....js` (1). No `style-src-attr` report appears because the `innerHTML` assignment throws before parsing.
- Worker scope: no worker target was ever created. Page console: `Failed to construct 'Worker': This document requires 'TrustedScriptURL' assignment.` and `Could not create web worker(s). Falling back to loading web worker code in main thread, which might cause UI freezes.`
- Rendered: the editor object exists and the page status says `editor created`, but the DOM has 0 `.view-line`, 0 line numbers, 0 selection rectangles and no find widget or palette; the screenshot is an empty bordered rectangle. Typing: model unchanged (`hasFocus` false). The DiffEditor computes 3 line changes (main-thread fallback) and renders nothing.

### Variant 2, single policy and nonce shim

- Trusted Types: Monaco asked the hook for the same 10 names; the browser saw one `createPolicy('lanekeeper-monaco')`, accepted. 0 `trusted-types` reports, 0 `require-trusted-types-for` reports, 0 `style-src-elem` reports (the shim works), 0 page errors. Editor worker created through `createScriptURL` from `?worker&url` and attached; it computed the DiffEditor's 3 line changes and the unicode highlight decorations.
- Remaining directive: `style-src-attr`, blocked `inline`, sample empty, source the `innerHTML` call in the view layer. 171 at load in the editor, 373 in the diff editor, 177 in the languages run; by stage in the editor run: load 171, typing 116, select all 124, scroll 263, find 184, 4,000-line sweep 3,802.
- Rendered, correct: text and line numbers at first paint (29 of 29 lines within 2 px although their HTML carried a blocked `style` attribute; consistent with a later CSSOM write of `top`, not traced), syntax tokens, caret, typing (model `zz: 1`, DOM shows it), find widget (opens, `1 of 11`), F1 palette (600x271).
- Rendered, broken: current-line highlight 4 px wide (823 expected); indent guides 0 of 6 boxes non-zero; Ctrl+A creates 40 selection rectangles, none with a non-zero size (the first three are 0x0; the screenshot shows a few grey discs of varying size at scattered positions and no blue selection over the text); the unicode-highlight boxes are 2x19 px (8x19 and 63x19 expected); after `setScrollTop(500)` only lines 27 to 30 are drawn and the rest of the viewport is blank (30 `.view-line` with 5 distinct tops, 4 within 2 px of the right place); over 24 scroll positions of a 4,000-line document 1 is correct and the worst error is 68,400 px; a 12-line typing burst leaves 2 of 30 lines misplaced (worst 228 px); 28 distinct tops for 30 line numbers at load. DiffEditor: 3 line changes computed, but 2 of 10 decoration boxes are non-zero, no line backgrounds or character highlights drawn, no indent guides.

### Variant 3, option (e) as the plan describes it

- 0 reports of any directive in every run (editor, diff, languages; features default and off); 0 page errors; policies created: `lanekeeper-monaco` only; worker attached. The rewrite applied 171 style attributes at load in the editor (373 in the diff editor, 177 in the languages run), the same numbers that variant 2 reports as `style-src-attr`, with 0 failures: `el.style.cssText = value` is accepted by the browser under this CSP.
- Rendering: every number in tables T2 to T4 equals the expected value (lines 29 of 29 within 2 px at load and after typing, 30 of 30 after scroll, 24 of 24 positions of the 4,000-line sweep, burst 30 of 30, selection 40 of 40 boxes non-zero, current line 823 px, indent guides 6 of 6, unicode highlight 8x19 and 63x19, DiffEditor line changes 3, decoration boxes 10 of 10). Languages: yaml 5 token classes, json 6, xml 6, ini 6, plain text 1.
- Layout read right after an `innerHTML` insertion (T7): an element inserted with `height:50px` reports 0 immediately, 50 after one microtask. Monaco tolerated this in every scenario measured. The synchronous pass (3b) reports 50 immediately.
- Three places where variant 3 is not clean (T5, T6):
  1. DiffEditor inline mode: 3 `style-src-attr` reports, one per deleted line. Source: `marginElement.setAttribute('style', ...)` (`diffEditorViewZones.js:199`), which the `createHTML` hook cannot see. With `setAttribute('style')` routed to `cssText` (3c, 3d): 0, and `setAttributeIntercepted` is 3. On the editor page (hover, suggest, context menu, go to line, folding, wrapping, mouse selection, multi-cursor, find and replace, screen-reader mode; 1,228 and 1,235 style attributes rewritten in the 3c and 3d runs) `setAttributeIntercepted` ends at 0, so no other `setAttribute('style')` call happened in the scenarios run. A static search of `base/`, `editor/` and `platform/` of 0.57.0 finds that one call.
  2. Markdown paths. A hover provider returning markdown (variant 3, features default): 3 reports (`trusted-types` for `dompurify`, `DOMParser.parseFromString`, `Element innerHTML` with sample starting `<p`) and the hover box (60x27 px) contains the text `null`. A completion item with markdown documentation, after Ctrl+Space twice: 2 reports (the two sinks; the policy name was already refused) and the details pane (332x40) shows `null`. With the plan's feature set off, hover shows no widget and has 0 reports, but Ctrl+Space with such a provider still produces 3: the option does not remove the command.
  3. Text corruption (T6): for model lines whose text contains `style="..."` or `style='...'` or `style = "..."`, the regex also matches inside the escaped text, so the editor displays `data-lk-style="..."` where the model has `style="..."`. 4 of 5 test lines differed. The model value is unchanged; the view is wrong.

### Variant 3d, a sound shape of (e)

Tag-aware rewrite (only inside real tags), the observer pass, `setAttribute('style')` routed to CSSOM, one policy, nonce shim, worker through the policy. 0 reports in every run of the matrix (editor, diff and languages; features default and off), 0 in every interaction stage including DiffEditor inline mode, 0 text mismatches, all rendering numbers as in variant 3. Still failing, as in variant 3: the markdown paths (hover 3 with hover on, suggest documentation 2 with features default and 3 with features off).

### Option (a) data point

Variant 2 served with the exact CSP plus `style-src-attr 'unsafe-inline'`: 0 reports in the editor, diff and languages runs and in every interaction stage except the same markdown paths (hover 3, suggest documentation 2); rendering numbers as in variant 3d (T2 values: 29 of 29, 30 of 30 after scroll, 24 of 24 positions, selection 40 of 40, current line 823 px, indent guides 6 of 6, unicode highlight 8x19 and 63x19, DiffEditor decoration boxes 10 of 10, 3 line changes).

## Results tables (generated from the per-run JSON)

"At load" is page load plus 2.5 s of settling. "All stages" adds the scripted interaction (typing, select all, scroll, find, F1 palette, a 4,000-line document with a 24-position scroll sweep, a typing burst). Counts for variant 2 differ by a few percent between runs because the sweep and the observer timing are not deterministic.

### T1 violations per run

| run (variant / kind / features) | at load | all stages | by directive (load stage) | TT policies refused | TT policies created | page errors | editor worker attached | lines rendered |
|---|---|---|---|---|---|---|---|---|
| 1 / editor / default | 23 | 60 | TT policy 10, style-src-elem 3, TT sink innerHTML 9, TT sink Worker 1 | 10 | none | 56 | no | 0 |
| 1 / editor / off | 23 | 57 | TT policy 10, style-src-elem 3, TT sink innerHTML 9, TT sink Worker 1 | 10 | none | 53 | no | 0 |
| 2 / editor / default | 171 | 4709 | style-src-attr 171 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 2 / editor / off | 171 | 4661 | style-src-attr 171 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3 / editor / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3 / editor / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3b / editor / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3b / editor / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3c / editor / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3c / editor / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 1 / diff / default | 29 | 29 | TT policy 10, style-src-elem 4, TT sink Worker 1, TT sink innerHTML 14 | 10 | none | 24 | no | 0 |
| 1 / diff / off | 30 | 30 | TT policy 10, style-src-elem 4, TT sink Worker 1, TT sink innerHTML 15 | 10 | none | 25 | no | 0 |
| 2 / diff / default | 373 | 373 | style-src-attr 373 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 2 / diff / off | 373 | 373 | style-src-attr 373 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3 / diff / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3 / diff / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3b / diff / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3b / diff / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3c / diff / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3c / diff / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 1 / langs / default | 24 | 50 | TT policy 10, style-src-elem 3, TT sink innerHTML 10, TT sink Worker 1 | 11 | none | 46 | no | 0 |
| 2 / langs / off | 177 | 470 | style-src-attr 177 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3 / langs / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3c / langs / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3t / editor / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3t / diff / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3d / editor / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3d / editor / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3d / diff / default | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3d / diff / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |
| 3d / langs / off | 0 | 0 | 0 | 0 | lanekeeper-monaco | 0 | yes (1) | 30 |

### T2 editor rendering and behaviour (kind editor, plan feature set off)

| variant | lines at right top (within 2 px, initial) | current-line box w (px) | indent guides non-zero / total | Ctrl+A rects non-zero / total | after scroll to 500: distinct tops of 30 / within 2 px | 4000-line sweep: positions within 2 px of 24 (worst dev px) | typing burst within 2 px | typed text in model and DOM | unicode highlight box (w x h px) | find widget | F1 palette |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 plain | 0 lines in the DOM | n/a | 0 | 0 | 0 | n/a (no lines to measure) | n/a | typed: model unchanged, no focus (hasFocus false) | none (3 decorations, 0 in DOM) | not created | not created |
| 2 single policy + nonce shim | 29 / 29 | 4 | 0 / 6 | 0 / 40 | 5 / 4 | 1 / 24 (68400) | 28 / 30 | yes | 2x19 and 2x19 | opens, 1 of 11 | opens 600x271 |
| 3 option (e) | 29 / 29 | 823 | 6 / 6 | 40 / 40 | 30 / 30 | 24 / 24 (0) | 30 / 30 | yes | 8x19 and 63x19 | opens, 1 of 11 | opens 600x271 |
| 3b (e) + sync pass | 29 / 29 | 823 | 6 / 6 | 40 / 40 | 30 / 30 | 24 / 24 (0) | 30 / 30 | yes | 8x19 and 63x19 | opens, 1 of 11 | opens 600x271 |
| 3c (e) + sync + setAttribute | 29 / 29 | 823 | 6 / 6 | 40 / 40 | 30 / 30 | 24 / 24 (0) | 30 / 30 | yes | 8x19 and 63x19 | opens, 1 of 11 | opens 600x271 |
| 3t (e), tag-aware | 29 / 29 | 823 | 6 / 6 | 40 / 40 | 30 / 30 | 24 / 24 (0) | 30 / 30 | yes | 8x19 and 63x19 | opens, 1 of 11 | opens 600x271 |
| 3d (e), tag-aware + setAttribute | 29 / 29 | 823 | 6 / 6 | 40 / 40 | 30 / 30 | 24 / 24 (0) | 30 / 30 | yes | 8x19 and 63x19 | opens, 1 of 11 | opens 600x271 |

### T3 DiffEditor (side by side, plan feature set off)

| variant | line changes computed (expect 3) | modified side: decoration boxes non-zero / total | modified side: indent guides non-zero / total | current-line width (px) | lines at right top (modified / original) |
|---|---|---|---|---|---|
| 1 plain | 3 | 0 / 0 | 0 / 0 | n/a | 0/0 / 0/0 |
| 2 single policy + nonce shim | 3 | 2 / 10 | 0 / 6 | 4 | 29/29 / 28/28 |
| 3 option (e) | 3 | 10 / 10 | 6 / 6 | 436 | 29/29 / 28/28 |
| 3b (e) + sync pass | 3 | 10 / 10 | 6 / 6 | 436 | 29/29 / 28/28 |
| 3c (e) + sync + setAttribute | 3 | 10 / 10 | 6 / 6 | 436 | 29/29 / 28/28 |
| 3t (e), tag-aware | 3 | 10 / 10 | 6 / 6 | 436 | 29/29 / 28/28 |
| 3d (e), tag-aware + setAttribute | 3 | 10 / 10 | 6 / 6 | 436 | 29/29 / 28/28 |

### T4 languages (kind langs)

| variant | violations at load / all | page errors | yaml | json | xml | ini (.properties) | plaintext |
|---|---|---|---|---|---|---|---|
| 1 plain (default) | 24 / 50 | 46 | yaml: 0 token classes, 0 lines | json: 0 token classes, 0 lines | xml: 0 token classes, 0 lines | ini: 0 token classes, 0 lines | plaintext: 0 token classes, 0 lines |
| 2 single policy + nonce shim (off) | 177 / 470 | 0 | yaml: 5 token classes, 30 lines | json: 6 token classes, 6 lines | xml: 6 token classes, 6 lines | ini: 6 token classes, 6 lines | plaintext: 1 token classes, 3 lines |
| 3 option (e) (off) | 0 / 0 | 0 | yaml: 5 token classes, 30 lines | json: 6 token classes, 6 lines | xml: 6 token classes, 6 lines | ini: 6 token classes, 6 lines | plaintext: 1 token classes, 3 lines |
| 3c (e) + sync + setAttribute (off) | 0 / 0 | 0 | yaml: 5 token classes, 30 lines | json: 6 token classes, 6 lines | xml: 6 token classes, 6 lines | ini: 6 token classes, 6 lines | plaintext: 1 token classes, 3 lines |
| 3d (e), tag-aware + setAttribute (off) | 0 / 0 | 0 | yaml: 5 token classes, 30 lines | json: 6 token classes, 6 lines | xml: 6 token classes, 6 lines | ini: 6 token classes, 6 lines | plaintext: 1 token classes, 3 lines |

### T5 interaction stages (violations caused by each stage; hover and suggest have a markdown provider registered)

| stage | 2 | 3 | 3c | 3d | 3d, features off |
|---|---|---|---|---|---|
| hover-markdown | 5 | 3 | 3 | 3 | 0 |
| suggest-docs | 120 | 2 | 2 | 2 | 3 |
| context-menu | 8 | 0 | 0 | 0 | 0 |
| goto-line | 135 | 0 | 0 | 0 | 0 |
| folding | 132 | 0 | 0 | 0 | 0 |
| unfold-and-wrap | 259 | 0 | 0 | 0 | 0 |
| mouse-selection | 289 | 0 | 0 | 0 | 0 |
| multi-cursor | 52 | 0 | 0 | 0 | 0 |
| find-replace | 0 | 0 | 0 | 0 | 0 |
| screen-reader-mode | 140 | 0 | 0 | 0 | 0 |
| diff: inline-mode | 71 | 3 | 0 | 0 | 0 |
| diff: accessible-diff-viewer | 89 | 0 | 0 | 0 | 0 |

The accessible diff viewer did not open (no action is registered, see "Not measured"); the counts of the last row for variant 2 come from switching back from inline to side-by-side mode. The hover and suggest stages count the markdown path plus, for variant 2, its style attribute reports.


### T6 text corruption by the createHTML rewrite (lines whose text contains style="...")

| variant | model lines | lines whose displayed text differs from the model | violations |
|---|---|---|---|
| 2 single policy + nonce shim | 5 | 0 | 155 |
| 3 option (e) | 5 | 4 | 0 |
| 3t (e), tag-aware | 5 | 0 | 0 |
| 3d (e), tag-aware + setAttribute | 5 | 0 | 0 |

### T7 layout read right after innerHTML insertion (element styled with height:50px)

| variant | immediately | after microtask | after animation frame |
|---|---|---|---|
| 2 single policy + nonce shim | 0 | 0 | 0 |
| 3 option (e) | 0 | 50 | 50 |
| 3b (e) + sync pass | 50 | 50 | 50 |
| 3c (e) + sync + setAttribute | 50 | 50 | 50 |
| 3t (e), tag-aware | 0 | 50 | 50 |
| 3d (e), tag-aware + setAttribute | 0 | 50 | 50 |

## Findings that bear on the plan

1. **F7 is confirmed in kind and refined in scope.** Style attributes assigned through `innerHTML` and `insertAdjacentHTML` are reported (171 at load) and the overlay boxes that depend on them lose their size (T2, T3). First-paint lines are at the right top (29 of 29) although their HTML carried a blocked `style` attribute, which is consistent with a later CSSOM write of `top` (not traced); lines that Monaco inserts after a scroll lose their `top` (T2). The `diffEditorViewZones.js:199` call is a `setAttribute`, reached only in DiffEditor inline mode (3 reports). The policy names `defaultWorkerFactory`, `diffEditorWidget`, `diffReview`, `domLineBreaksComputer`, `editorGhostText`, `editorViewLayer`, `richScreenReaderContent`, `standaloneColorizer`, `stickyScrollViewLayer`, `tokenizeToString` are all requested at module evaluation; DOMPurify's own request is `dompurify` and happens lazily on a markdown path; `domSanitize<N>` was not observed in any run.
2. **Workers.** Without `MonacoEnvironment.getWorker`, 0.57.0 builds a `blob:` module worker from a bootstrap Blob (`webWorkerServiceImpl.js`; read, not exercised here), which `worker-src 'self'` does not allow. Vite's `?worker` wrapper passes a plain string to `new Worker` and is a `require-trusted-types-for` sink (variant 1). `?worker&url` plus `createScriptURL` works, with the iife single-file worker, under the API policy of `/assets/*` (`default-src 'none'`).
3. **The plan's `e2e/csp` listener** ("fails on any" console error, Log or Audits event) would fail on a CSP-clean page, because of three events that are not CSP violations: Chromium 141 logs `Error with Permissions-Policy header: Unrecognized feature: 'bluetooth'.` (console warning, `Log` source `security`) for the `Permissions-Policy` list in `headers.rs`; the first page load in a browser requests `/favicon.ico` and gets a 404 (`Failed to load resource`, console error; `index.html` needs a `<link rel="icon">`, and `img-src 'self' data:` allows a `data:` icon); and `Audits` reports `GenericIssue` events of type `FormEmptyIdAndNameAttributesForInputError` and `FormLabelHasNeitherForNorNestedInput` (8 and 11 on the editor page after the interaction script; 2 and 0 on a freshly loaded diff page; Monaco's inputs). Filtering on `ContentSecurityPolicyIssue`, `securitypolicyviolation` and Trusted Types errors avoids all three.
4. **Bundle (answers the last bullet of F7).** About 4.0 MB raw (1.02 MB gzip -9) for the editor chunk with all contributions, plus 175 KB CSS, 275 KB worker, 153 KB font; the hub sends no compression.
5. **Unicode highlighting (S8) works with the single editor worker** in variants 3 and 3d: 3 decorations after 0.4 s, boxes 8x19 and 63x19 px for the zero-width space and the right-to-left override test characters (the third decoration, a Cyrillic letter, is outside the viewport). In variant 2 the same boxes are 2x19 px.

## What the numbers say about each W3 option

| Option | Measured here | Reports | Notes from the measurements |
|---|---|---|---|
| (a) `style-src-attr 'unsafe-inline'` | yes, variant 2 plus that directive | 0 in editor, diff, languages and all stages; markdown paths 3 and 2 | everything else in the plan's environment rules still required (one policy, nonce shim, worker through the policy). Variant 2 without the directive: only `style-src-attr` is reported (171 at load) |
| (b) CodeMirror 6 | no | n/a | not measured |
| (c) textarea edit mode | no | n/a | not measured |
| (d) read-only first | no | n/a | not measured |
| (e) as the plan states it (variant 3) | yes | 0 in editor, side-by-side diff, languages and all stages except inline diff (3), markdown (3 and 2) | regex corrupts displayed text (4 of 5 lines); layout reads right after insertion see unstyled boxes for one microtask |
| (e) sound shape (variant 3d) | yes | 0 everywhere measured except the markdown paths (3 and 2) | needs a tag-aware rewrite, the observer pass, a global `setAttribute('style')` routing, a global `createElement` nonce shim; markdown-rendering contributions should be left out of the entry file (turning hover off does not stop Ctrl+Space) |

## G0 recommendation (agent; the decision is the human's)

Option (e) in the shape of variant 3d is the only measured way to run Monaco with the production CSP unchanged and with 0 reports in the exercised scope; option (a) is the measured fallback with the same rendering and a one-directive relaxation. If (e) is chosen, the plan's description must change from a regex over `style="..."` to a tag-aware rewrite plus `setAttribute('style')` routing, and `src/editor/` must import only the contributions it needs (leave out the ones that render markdown: hover, suggest, parameter hints, inline completions, colour picker, links, code lens, inlay hints, goto-symbol peek). The G0 stage list (T5) should become `e2e/editor/csp.spec.ts` so that every Monaco upgrade is re-measured, because (e) depends on Monaco's internal HTML strings and the one `setAttribute` call.

## Not measured

- Cmd/Ctrl+K chord and the global shortcut layering (F7 bullet), IME composition, touch, high-contrast theme, right-to-left text.
- Firefox and Safari (no browser available; only Chromium 141).
- The real hub binary and `static_files.rs` (the headers were reproduced by a Node server).
- Monaco's accessible diff viewer: no action matching `accessibleDiff` or `diffReview` is registered in the standalone modified editor (F7 does nothing), so the `diffReview` policy is requested at load but the viewer was not exercised.
- Timing or memory cost of the observer beyond the sweep (no latency figures were taken).
- Behaviour on Monaco versions other than 0.57.0.

## Reproduce

Harness (outside the repository, not committed): `/tmp/claude-0/-home-user-lanekeeper/44c626c4-c7b9-55f1-aa0f-edfeb3376d24/scratchpad/g0/`. Raw per-run JSON and 176 screenshots are in its `results/`.

```
npm install --save-exact --ignore-scripts vite@6.4.3 monaco-editor@0.57.0   # package.json "overrides": rollup 4.63.5, nanoid 3.3.19, postcss 8.5.28
node tools/gen-entry.mjs "$PWD/node_modules/monaco-editor/esm/vs" "$PWD/src/monaco-entry.js"
npx vite build                      # dist/ (variants 1, 2, 3, 3b, 3c); npx vite build --outDir dist2 (adds 3t, 3d, corruption probe)
node tools/server.mjs dist 4173 <repo>/crates/hub/src/headers.rs         # EXTRA_CSP="style-src-attr 'unsafe-inline'" on 4175 for option (a)
node tools/measure.mjs results http://127.0.0.1:4173 "$(cat runs.json)"   # matrix: variant x kind x features
node tools/extras.mjs  results http://127.0.0.1:4173 '["2","3","3c"]'     # interaction stages (hover, suggest, menu, folding, wrap, ...)
node tools/corrupt.mjs results http://127.0.0.1:4174 '["2","3","3t","3d"]'
node tools/report-tables.mjs        # prints T1 to T7
```

## Appendix: variant 3d, the code that matters

```js
// createHTML of the one policy: rewrite style attributes that sit inside tags only
export function rewriteStyleAttrsInTags(html) {
  let out = '', i = 0; const n = html.length;
  while (i < n) {
    const lt = html.indexOf('<', i);
    if (lt === -1) { out += html.slice(i); break; }
    out += html.slice(i, lt);
    if (!/[A-Za-z/!]/.test(html[lt + 1] ?? '')) { out += '<'; i = lt + 1; continue; }
    let j = lt + 1, quote = null;                       // scan to the closing '>' honouring quoted values
    while (j < n) { const ch = html[j]; if (quote) { if (ch === quote) quote = null; } else if (ch === '"' || ch === "'") quote = ch; else if (ch === '>') break; j++; }
    out += html.slice(lt, j + 1).replace(/(\s)style\s*=\s*(?:"([^"]*)"|'([^']*)')/gi,
      (_m, ws, dq, sq) => `${ws}data-lk-style="${dq !== undefined ? dq : String(sq).replaceAll('"', '&quot;')}"`);
    i = j + 1;
  }
  return out;
}
// CSSOM pass (applyTree finds [data-lk-style] under each added element): el.style.cssText = value; el.removeAttribute('data-lk-style')
new MutationObserver((records) => { for (const r of records) for (const n of r.addedNodes) if (n.nodeType === 1) applyTree(n); })
  .observe(document, { childList: true, subtree: true });
// the one direct sink of the core editor (DiffEditor inline mode)
const setAttr = Element.prototype.setAttribute;
Element.prototype.setAttribute = function (name, value) {
  if (typeof name === 'string' && name.toLowerCase() === 'style') { this.style.cssText = String(value); return; }
  return setAttr.call(this, name, value);
};
// nonce shim for <style> elements created by Monaco's createStyleSheet
const nonce = document.querySelector('meta[property="csp-nonce"]')?.nonce ?? '';
const create = Document.prototype.createElement;
Document.prototype.createElement = function (tag, ...rest) { const el = create.call(this, tag, ...rest); if (typeof tag === 'string' && tag.toLowerCase() === 'style') el.nonce = nonce; return el; };
```

## Decisions and handoff (recorded 2026-10-17)

This section records the decisions the human gave in the chat on 2026-10-16 and 2026-10-17, which exist nowhere else in the repository, and the state of the branch, so that a fresh session in another environment can resume prompt 08 at T1 with the same facts. `plans/08-frontend.md` is frozen and was not edited. Facts were checked against the repository on 2026-10-17 unless a line says otherwise. "The human" is the person who approved the plan.

### Decisions

1. **Plan approval (plan section "Extra paths and contract changes", W14).** The human approved revision 1 of `plans/08-frontend.md` (commit 514c48a, `08/plan: approved by a human`) without `--contract-change`. This is the W14 default. The five Extra-paths take effect: `crates/hub/examples/web_fixture.rs`, `.github/workflows/web-e2e.yml`, `docs/threat-model.md`, `crates/xtask/tests/budgets_registry.rs` and `perf/budgets.toml`. The plan header still reads `Contract-change: needed`, so every other contract path stays locked on this branch. Decided by the human (the approval commit is dated 2026-10-16).
2. **W1, the contract PR.** The human decided on one separate additive PR, written by agents and planned in `plans/contract-08-web.md` (on `main`). It was merged as PR #20, `contract: web app additions` (merge commit 4d9eca8 on `main`, branch tip c1b255f). This branch merged it in 5e9038e, which also brought in the merged work of prompts 05 and 06.
   - **In the PR:** CG1a, CG1b, CG2, CG3, CG4a, CG5, CG7 (as M1, see item 5), CG10, CG11, CG12, CG13, CG14 and CG17, plus the fixes that make `SwimlaneDetail`, `Me`, `SeverityRule` and `RetentionSettings` validate real bodies.
   - **Not in the PR, so still open for 08:** CG4b (lock cause), CG6 (adoption operations), CG8 (rendered text of resource files and served-file comparisons), CG9 (swimlane activity feed and viewer-scoped audit) and CG15 (admin swimlane writes).
   - **What that means for the tasks.** A slice whose item is still open ends BLOCKED or uses a `design/DEVIATIONS.md` row, as the 08 plan says. The adoption wizard (T4 S6b) needs a later contract PR, `contract/08-adoption`. "As served" (T6) needs `contract/08-served` before T6. The fallbacks in `plans/contract-08-web.md` are: CG4b, derive the lock reason from the operation and `baseline`; CG9, an Admin-only Activity tab through `adminListAudit` (W11); CG15, no register-swimlane form. Decided by the human in the chat; the scope is also recorded in `plans/contract-08-web.md`, section "Decisions".
3. **W2, fan-out.** The human accepted the plan's serial fallback. Finding: an implementer subagent cannot spawn sub-agents (the T0 implementer confirmed it has no Agent tool), so T0 ran its slices serially. For T4 the human chose orchestrator-driven slices: the orchestrator spawns up to 4 slice agents at a time on disjoint files. Slices never commit and never edit the plan's shared files. An implementer then integrates, runs the Verify and makes the single `08/T4` commit. T1 to T3, T5 and T6 default to one implementer working the slices serially unless the human says otherwise. Decided by the human in the chat.
4. **W3, Monaco.** After G0 (the report above), the human chose option (e), in its tag-aware shape (G0 variant 3d): one Trusted Types policy, `lanekeeper-monaco`; the nonce shim for `<style>` elements; a tag-aware `createHTML` rewrite of `style="..."` into a data attribute that a `MutationObserver` applies through CSSOM; `setAttribute('style')` routed to CSSOM; and the worker created through `createScriptURL`. The CSP is not weakened and no `style-src-attr 'unsafe-inline'` is added, so W4 does not arise. G0 measured 0 violation reports for the editor, DiffEditor in inline mode and the five languages, in Chromium 141 only. Markdown hover and suggest documentation still fail under the policy (G0 findings, "Variant 3d"), so those features stay switched off; G0 also advises leaving the contributions that render markdown out of the entry file, because turning hover off does not stop Ctrl+Space. Monaco is pinned to 0.57.0 with regression tests (G0 proposes `e2e/editor/csp.spec.ts`, which repeats the G0 stages on every upgrade), because the mechanism depends on Monaco's internal HTML strings and its one `setAttribute` call. Decided by the human in the chat, after the G0 report (committed 2026-10-16 16:16 UTC).
5. **W5, masking.** The human chose M1, and it is in the merged contract. `FileContent`, `ServedFile` and `DocText` carry an optional `masks {line, start, end, setting?}`, with UTF-8 byte offsets relative to the `text` of the same object. `IssueKind` has the new value `redaction_marker`, and a write whose content contains a marker is refused with 422 `invalid_content`. T5's masking work and Edit mode for files that may hold flagged values are therefore unblocked at the contract level. The server owners must still build it: 05 (the REST edge), 06 (the marker refusal) and 14 (docs). The web app treats the contract as the guarantee and never masks by itself. Edit mode stays off while `masks` is not empty. Decided by the human in the chat; the choice is recorded in git at dd98c87 (2026-10-16).
6. **Findings from the contract work that bind 08.** They are recorded in `docs/interfaces.md`, section "Web additions" (the open items there). They are requirements on the web client, not choices the human made for 08.
   - The web app must not percent-decode a doc link. `/%2F%2Fevil.example/` is a same-origin path to a browser, but a client router that decodes it would turn it into `//evil.example/`. The app hands the `href` to the browser as the server sent it (open item 6).
   - `renderDoc` output (`RenderedDoc.html`) is the one HTML sink the CSP lets through (CG11, S12).
   - The `img src` rule of the rendered-docs allowlist is any same-origin path today, and owner 14 may tighten it to a docs-asset prefix. The renderer and the web app must not assume that any same-origin path is safe to fetch (open item 4).
   - Text search is an oracle for a hidden value (owners 04, 05 and 14), and compare and grid answers say whether two hidden values are equal (open items 1 and 3). The UI must not imply that a masked value cannot be probed.
7. **T1 start conditions.**
   - T1 generates its API client from the merged `api/openapi.yaml` (99 operations, counted on 2026-10-17). After `pnpm gen:api`, expect an orval override for the `text/event-stream` schema of `streamEvents` (`docs/interfaces.md`, the 08 row).
   - T1 bumps `vitest` to an exact 4.x pin that is at least 14 days old, to clear the 5 `pnpm audit` findings that were already in the baseline lockfile (tinypool, source-map-js, vitest and @vitest/mocker). If the tests break, T1 records a blocker with measurements instead. Checked here: `web/package.json` has `"vitest": "^3.2.7"`, and the lockfile pins vitest 3.2.7, @vitest/mocker 3.2.7, tinypool 1.1.1 and source-map-js 1.2.1, the same versions as in the scaffold commit ddf29a3. The count of 5 findings comes from an audit run during T0 and was not re-run for this record.
   - `just verify-08` is still the stub (`exit 1`, `Justfile` line 530). T1 replaces it, as the plan says.
8. **Open questions the human has not answered.** The plan's defaults stand for all of them. Those marked "Needs a human" in the plan stay open: W6, W7 (design owner: handoff v2, the visual tiers, and the 0.2 per-pixel and 0.5% pixel threshold), W8 (keep `@playwright/test@1.63.0` with `executablePath`, or pin 1.56.1), W11, W12, W13, W15 to W20, W27, W28, W30 and W36. Answer or confirm each one before the task that depends on it. W10, W23 to W25 and W35 are design-owner confirmations that become `DEVIATIONS.md` rows.
9. **Recorder's notes for T1 (observations from reading the frozen plan against the decisions; not decisions).**
   - The plan's option (e) describes a plain rewrite of `style="..."`; decision 4 makes it tag-aware and adds global patches (`Element.prototype.setAttribute`, `Document.prototype.createElement` and a `MutationObserver` on `document`) that load with the lazy Monaco chunk. The plan's lint list (which bans `setAttribute('style')` and `trustedTypes.createPolicy` outside `src/lib/csp`) and its threat-model text do not mention these patches. T1 should check its lint rules against the G0 appendix code, add a named and documented override if one is needed, and record the patches in `docs/threat-model.md` (an approved Extra-path).
   - G0 finding 3 applies to the plan's `e2e/csp` listener: it must filter on CSP issue events, `securitypolicyviolation` and Trusted Types errors, and `index.html` needs a `<link rel="icon">`.

### Environment notes for resuming elsewhere

- **The sandbox the first part ran in.** About 14 to 20 GB of free disk (it fell to 125 MB twice during Rust builds), 4 cores and 15 GB of memory. Rechecked on 2026-10-17: 17 GB free, 4 cores, 15 GB of memory.
- **Missing tools there.** No `just`, `cargo-deny`, `cargo-audit` or `osv-scanner`, and no Docker daemon (the `docker` client exists but the daemon is not reachable). So `just verify-08` and `just verify` cannot run there, and CI is the authority.
- **Browsers.** Chromium 141 (Playwright build 1194, under `/opt/pw-browsers`) was preinstalled, while `@playwright/test@1.63.0` expects revision 1243 (Chromium 153). `cdn.playwright.dev` (the Playwright download host) and `ui.shadcn.com` (the shadcn registry) returned 403 through the proxy. The shadcn primitives are therefore hand-written, per plan W9.
- **Agents.** Implementer subagents had no Agent tool (decision 3).
- **Code-graph index.** The Graphify index was built at 777b590 and is stale: `origin/main` is now 4d9eca8.
- **For a fresh environment.** Rust 1.88.0 (`rust-toolchain.toml`, with rustfmt and clippy); Node 22 (22.22.0 was used here, and the plan raises `engines.node` to `>=22.18` for orval) with pnpm 10.34.6 (the `packageManager` pin); `just`; Docker (needed by the Rust prompts, not by 08's own gates); `cargo-deny` 0.19.9 and `cargo-audit` 0.22.2 (the versions the Justfile pins); `osv-scanner`; Playwright browsers that match the pinned `@playwright/test` version; and at least 30 GB of free disk.
- **Quiet machine.** Timing-based gates (the P10 numbers) and the visual baselines need a quiet machine. The plan generates the baselines in CI only.

### How to resume

1. Fetch the branch. `agent/08-frontend` is on origin at 5e9038e. The commit that adds this section is local until someone pushes it, so push it first if the other environment cannot see this worktree.
2. Create the worktree for it. In a clone whose main checkout is on `main`, run `git fetch origin`, then `git worktree add -b agent/08-frontend .worktrees/08-frontend origin/agent/08-frontend` (or, if the local branch already exists, `git worktree add .worktrees/08-frontend agent/08-frontend`). `node scripts/lk.mjs worktree 08` only reuses a branch that exists locally; with only `origin/agent/08-frontend` it would cut a new branch from `main`, which is wrong. If you check the branch out in the main checkout instead (`git checkout agent/08-frontend`), `lk` will not find `.worktrees/08-frontend` (its status shows `plan: -`), and git will refuse to add a second worktree for the same branch.
3. Run `node scripts/lk.mjs status` from the main checkout, not from inside the worktree. For prompt 08 it should show `plan: approved` and `taskCommits: 1/7` (T0 is done). From inside the worktree it prints `plan: -` and `0/7`, because `lk` looks for the worktree under its own root. The branch holds, after the scaffold, these commits: `08/plan: draft`, `08/plan: revise 1`, `08/plan: approved by a human`, `08/T0: import the design handoff ...`, `08/blocked: G0 Monaco under the production CSP (W3)`, the merge 5e9038e, and `08/decision: record the human's decisions and the handoff state`.
4. Run `/run-prompt 08`. It resumes the approved plan at the next task without a commit, T1, with a fresh `web-implementer`.
5. The orchestrator must read this section first, and the G0 report above it.
