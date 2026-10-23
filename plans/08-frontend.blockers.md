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

10. **A server-state layer, folded into T3 (human, 2026-10-17; the plan is frozen, so it is recorded here).** The human approved adding a TanStack Query server-state layer to T3's commit (`08/T3: live updates (SSE client, backoff, Last-Event-ID, precise invalidation) and the server-state layer`), in these words: domain-shaped query keys, colocated query functions, optimistic mutations where useful, targeted invalidation after writes, loading, error, empty, background-refetch and stale-data states, and server data kept out of global client state. T2 had already delivered `src/lib/query/{keys,cursor-pages,client,retry,secret-mutation}`; T3 extends them.
   - **Keys.** One factory per domain in `src/lib/query/keys.ts` (`meKeys`, `swimlaneKeys`, `fileKeys`, `compareKeys`, `gitKeys`, `findingKeys`, `proposalKeys`, `prKeys`, `docKeys`, `auditKeys`, `adminKeys`), with the plan's vocabulary of roots and nothing added to it. The live events, the write table and the query modules use them and nothing else: the lint rule `query-key-literal` and `src/lib/query/keys-usage.test.ts` prove it (no literal key, no generator key, no generated query hook).
   - **Query functions.** `src/features/<domain>/queries.ts` (and `mutations.ts` where writes exist) for access, swimlanes, file, compare, findings, approvals, prs, docs, status, audit and admin: `queryOptions` or `useCursorPages` options built from the generated functions and the key factories, each with its freshness rule (`src/lib/query/freshness.ts`: event-fed, polled, immutable). Not created, because the contract has no operation for them in the plan's key list: the doc reader, search, effective config, served files, drift list, restart offers. T4 slices import these.
   - **Writes.** `src/lib/query/invalidate.ts`: a table keyed by operationId, typed by the generated operation list (a mutating operation without a row does not compile; a test checks it again at run time). Every row is a set of targets built from the key factories, or `none` with a reason. A write that was sent for approval (`outcome: "proposal_created"`) refetches the queue and not the file. `invalidateQueries()` without a filter is a lint error and a source scan. Secret-bearing operations keep the T1 rules.
   - **Optimistic updates: a narrow exception to the plan's lint line.** The plan bans `onMutate` and `setQueryData` inside mutations. The human allowed ONE helper, `src/lib/query/optimistic.ts`, as the only code that touches the cache in a mutation's lifecycle (cancel, snapshot, apply, roll back on error, always refetch on settle), as a named one-file lint override (`optimistic-helper` in `web/lint/lanekeeper-rules.mjs`), never for a write that can answer 202 or 409 or carries an expected hash or If-Match, is secret-bearing or reveals. The allow-list is a typed registry, and `optimistic.test.ts` checks every entry against `api/openapi.yaml`. **The contract has exactly one honest candidate: `adminPutAutoNotify`** (a PUT of the whole value `{enabled}`; answers 200, 400, 401, 403, 404 only; no hash, no secret, Admin only). No other write is optimistic, and none was invented for a demo.
   - **States.** `useQueryState` and `QueryBoundary` (loading, error, empty, ready, and the overlays refreshing and stale, with the causes `age`, `live-paused` and `refetch-failed`), in the design's States vocabulary.
   - **Server data out of global client state.** Client-state libraries are lint-banned and `server-state.test.ts` fails if one is a dependency (it also reads the lockfile: none is there, not even transitively); no query persistence (`no-persist.test.ts`); a source scan finds no context or module-level variable typed with a generated response type, apart from three reviewed constants listed with a reason (the session projection of the `me` query, an empty array of ranges, the table of event validators). The scan reads declared types one file at a time and says that it is an approximation.
   - **What T3 found in the contract and the hub, for the owners.** (a) The contract has 17 public event kinds (`swimlane_changed` was added by CG1b after the plan counted 16); the plan's invalidation table has no row for it, so `swimlane_changed` invalidates the swimlane, the list, the admin swimlanes and that swimlane's admin tenants and auto-notify entries. (b) A resume uses the last id received and the hub's ids are "not necessarily the order they committed" (`wire.rs`): an event that commits late with a lower id is not replayed. Recorded as a residual risk in `docs/threat-model.md`. (c) The grid's `swimlanes` parameter is comma separated (style form, explode false), and the generated client writes it so; the grid answers `Grid {swimlanes, rows}`, so the grid's pages are its `rows`.
   - **What the stream changed in T2's gates.** A signed-in page now keeps a stream open, so `networkidle` never settles and the first connection refetches what is on screen once. The engine (parser, client, invalidation plan) is a lazy chunk (`live-*.js`, 4.76 KB gzipped, fetched only for a signed-in user with a role, gated by `LAZY_APP_MODULES` in `scripts/perf/check-bundle.mjs`), so initial JavaScript went from 131.0 to **133.7 KB** gzipped (137.8 KB before the engine was made lazy). The T2 e2e specs wait for the stream and for its first refetch with `settleLive` (`web/e2e/perf/helpers.ts`) before they count requests or time a screen; no budget or threshold changed. P10 after T3, `cargo xtask bench-check` on the five ids: 5 pass (initial JS 133.7 KB of 250, route change p95 28.8 ms of 100, diff p95 187.2 ms of 500, grid capacity min 201.4 fps of 60, Lighthouse 95 of 90).
   - **For T4.** Invalidating an infinite query refetches every page loaded so far; the real grid route should use a first-page and reset refetch policy and ask for 50 rows first.

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

## T2 blocked: the 5,000-row grid frame-rate gate (`P10.grid_5000_rows_fps`, W20), recorded 2026-10-17

**Resolved the same day: see "W20 decided" at the end of this file.** The text below is the record of the blocker as it stood; its "What the human has to decide" and "How to finish T2" are answered there.

**Status.** Everything in T2 is built, tested and measured except one gate: the grid's 60 fps, with the metric exactly as W20 defines it, cannot be met reliably on this machine, and the same box fails it with a hand-written list that has no application code. The threshold is not changed, no gate is skipped, and nothing is committed as T2 (the rule is: commit only this file). All of T2's work is in the worktree `.worktrees/08-frontend`, uncommitted, and verified as listed below.

### What was measured, as W20 defines it

Five fresh pages, a 5 s programmatic scroll over 5,000 rows by 40 swimlanes (loaded through `useCursorPages` and the generated `getGrid`), `fps = min(60, 1000 / mean rAF interval)` rounded to 0.1, the minimum of five runs, in a normal (vsync-limited) headless Chromium 141.0.7390.37 (Playwright 1.63.0 with `executablePath`), software compositing, a 4-core virtual machine, load average 1.6 to 3.3.

| Execution of `e2e/perf/grid-5000.spec.ts` | `values.min` | Runs (fps) | Dropped frames (an interval over 25 ms) |
|---|---|---|---|
| 1 | 60.0 | 60, 60, 60, 60, 60 | 0 |
| 2 | 59.8 | 60, 60, 60, 60, 59.8 | one, at frame 4 |
| 3 | 59.8 | 60, 59.8, 60, 60, 60 | one, at frame 5 |
| 4 | 59.8 | 60, 60, 60, 59.8, 60 | one, at frame 3 |
| 5 (the `web-perf` steps end to end) | 59.2 | 59.4, 60, 59.2, 60, 60 | 3 and 4: frames 3, 5, 6 and 3, 104, 139, 175 |

Earlier builds of the same page measured 59.6 to 60.0. Every miss is a single 33 ms frame, which is 59.8; there are no long tasks in any run and the editor warm-up costs none. With the threshold at 60 and a rounding of 0.1, the gate needs ZERO dropped frames in 1,500.

### The control: the machine drops the same frames with no application code

`CONTROL_LIST_HTML` in `web/e2e/perf/helpers.ts`: a 140,000 px scroller with 40 absolutely positioned text rows that are moved and rewritten on every frame, no framework, scrolled the same way.

- 20 fresh pages each, alternating, same flags: the grid dropped at least one frame in **7 of 20** runs (8 frames in all, worst interval 50 ms); the control list in **7 of 20** (10 frames in all, worst interval 116.6 ms).
- Earlier, 10 runs each: the control list 3 of 10, a static 140,000 px page 0 of 10, a static 140,000 by 5,120 px page 1 of 10, the grid 4 of 10 (and 4 of 10 with the horizontal overflow removed).
- In execution 5 above, the control list in the same run scored a minimum of 58.4 (runs 59.8, 60, 60, 60, 58.4), under the grid's 59.2.
- A trace of a late frame shows the compositor thread (`ProxyImpl::ReadyToCommit`, `LayerTreeHostImpl::CommitComplete`, `TileManager::PrepareTiles`, `AssignGpuMemoryToTiles`) taking 31.6 ms at one commit and 10.4 ms at the next, which blocks the main thread's commit. It happens at the start of a scroll (frames 3 to 12) and at random places after it. Flags that move rasterisation to the GPU path (`--use-angle=swiftshader --enable-gpu-rasterization`) make it far worse here (every frame late), so they are not an answer.
- Taking the chance that a run is clean as about 0.65, five clean runs in a row is about 0.11: the gate as defined fails about nine executions in ten on this box, for any virtualised list.

### Headroom (informational, recorded in the result file under `headroom`)

The same grid, 300 frames a run with the same scroll step in every frame, in a Chromium launched with `--disable-frame-rate-limit --disable-gpu-vsync`: 178.6 to 256.4 fps per run (the minimum of five runs 178.6 in one execution and 192.3 in the last; mean 215 to 224), worst frame up to 34.9 ms, no long task. The page's own work per frame is about 4 to 6 ms of the 16.7 ms a frame has at 60 fps. What it does not see is a frame the compositor drops while the page is idle.

### What the human has to decide (W20, "Needs a human"; the agent has not chosen)

Any of these needs the spec and, for B, `perf/budgets.toml` (a contract path) to change, which the agent may not do alone.

- **A. Keep the metric and judge it on the hardware it was written for** (CI with a GPU-composited Chromium, or a quiet machine), with this box's number informational. Nothing in the repository changes except where the gate is run.
- **B. Amend the statistic** to tolerate a frame the machine drops: for example the median run instead of the minimum, or at most one dropped frame in 300. `statistic = "min"` in `perf/budgets.toml` would change.
- **C. Judge the page's own capacity**: the uncapped, equal-work-per-frame run above, with 60 as the floor (178.6 measured). It is blind to compositor stalls, so it would not catch a grid that makes the compositor slower.
- **D. Judge the grid against the control on the same machine in the same run** (for example "no more dropped frames than the control list plus a margin"). The spec already records both. It needs a rule for the margin.

The agent's reading: D together with C says the most (the page costs 4 to 6 ms a frame, and drops no more frames than the least a list can ask of this box), A is the most faithful to the original intent, and the unchanged metric on this machine is a gate that fails by luck.

### Done, verified, and uncommitted in the worktree

- Code splitting and loading: `lazyPage` (a bounded retry of a failed import, a stale-chunk flag, no `lazyRouteComponent`: it reloads the page and keeps its marker in `sessionStorage`, which loses unsaved work), `src/routes/pages.ts` (31 page loaders, each a chunk), intent preload and idle prefetch of the sidebar destinations, the "A new version is available" banner and screen (Reload held back while a screen has unsaved work), a fetch-only editor warm-up (the files come from a build-time `<meta>`, only `/assets/*.js|css`, at most eight, once, low priority), the editor fallback to `CodeView`.
- Data: `src/lib/query/keys.ts` (the key vocabulary), `cursor-pages.ts` (bounded keyset paging), `src/lib/text/byte-ranges.ts` (UTF-8 byte offsets to UTF-16, bounded, property-tested against `TextEncoder`).
- Views: `VirtualList`, `DataTable` (column windowing, pinned column, ARIA row and column counts, keyboard), `TreeView`, `CodeView`, `DiffView` (server hunks, split and unified, `n` and `p`, folds), `SafeCell`.
- Gates and tooling: `scripts/perf/{results,check-bundle,lighthouse-score,lighthouse}.mjs` with tests, `scripts/build/{chunk-hints,bundle-report}.mjs`, `web/lighthouserc.json`, `web-perf` and `web-e2e` in the `Justfile` (and `verify08.test.mjs`), the `chromium` and `perf` Playwright projects, `perf/budgets.toml` (five entries `registered = true`, no threshold touched) and `crates/xtask/tests/budgets_registry.rs` (the five ids), three rows in `design/DEVIATIONS.md`.
- Results with the browser and box they were measured on (Chromium 141.0.7390.37, 4 cores, load average 1.6 to 3.3): initial JavaScript **131.0 KB** gzipped of 250 (a single entry chunk; routes are small chunks); route change, cached, p95 **30.1 ms** of 100 (click 30.1, keyboard 25.3, cold 57.7 reported and not budgeted; 50 samples; one long task in the page over the whole spec); 6,000-line diff, cold, p95 **194.2 ms** of 500 (warm 77.5, revalidated 68.0, every line changed 219.1, all reported; the cold p95 was 179.7 in an earlier execution); Lighthouse 13.5.0 desktop, median of 3 per page, lowest page **95** of 90 (grid 96, diff 95, route page 98; stable to 1 to 3 points); grid **59.2** of 60 (the blocked gate). `cargo xtask bench-check` on the five ids: 4 pass, 1 fail (the grid). Noise: the route change and the diff passed with a margin of at least 2.5x in every execution, Lighthouse varied by 1 to 3 points, and only the 60 fps floor is inside the machine's noise.
- Checks run: `pnpm install --frozen-lockfile`, `pnpm lint`, `pnpm typecheck`, the full `vitest run` (85 files, 1,381 tests), `pnpm build` and `pnpm build:perf`, the `chromium` Playwright project (123 specs, among them the new `splitting`, `editor-warm`, `etag` and `csp-perf` specs: chunk loading, the stale-chunk screen without a reload, warm-up for an Editor only and from the cache afterwards, 304 revalidation, zero CSP violations), `pnpm gen:api` freshness (no change under `src/api`), `pnpm audit` (no known vulnerabilities), `cargo test -p xtask --test budgets_registry` (8 pass), `cargo fmt --check`. `just` itself is not installed here; its steps were run as commands, in the recipe's order.
- New dependencies, both exact pins at least 14 days old: `@tanstack/react-virtual@3.14.13` (published 2026-09-14; the fixed stack's virtualiser) and `lighthouse@13.5.0` (published 2026-09-18; plan W31's fallback, with a runner under `scripts/perf/`, because `@lhci/cli` brought 9 `pnpm audit` findings; the runner reads the same `lighthouserc.json` format and asserts the same median of 0.9). `lighthouse` needs Node 22.19 or later (22.22.0 here).

### Things the owners of other work should know

- **W13.** With the five P10 entries `registered = true`, a bare `cargo xtask bench-check` (every budget) fails until the web results exist in `target/perf-results/`. Whoever runs the full check (prompt 09's gate) must run `just web-perf` first.
- **The grid API is large.** One page of 500 rows by 40 swimlanes is 2.7 MB of JSON in the contract's shape (each cell repeats its setting path in `location`), and the hub sends it without `Content-Encoding` (W28). Under Lighthouse's simulated 10 Mbps a full first page made the grid page's LCP 3.2 s (score 82). The lab page now asks for 50 rows first and 500 after (LCP 1.2 s, score 96); the real route in T4 Phase C should do the same, and 05 and 07 may want to look at the cell shape.
- **Lighthouse pages in T2** are the perf build's test pages (grid, diff, route page), as the plan allows before the screens exist. T4 Phase C switches them to the real routes in `lighthouserc.json`.
- A Lighthouse run that records no navigation (`NO_NAVSTART`, seen once) is made again, at most twice, and only then: a low score is never repeated. The retry is unit-tested.

### How to finish T2 once W20 is decided

1. Change `e2e/perf/grid-5000.spec.ts` (and `perf/budgets.toml` for B, in a contract-change PR the human approves) to the chosen rule. The gated number is written as `values.min` of `web/grid_5000_rows_scroll_fps`; `headroom` and `control` are already recorded beside it.
2. Run the recipe's steps (`just web-perf`, then `just web-bench-check`), `just verify-08` and `just verify`.
3. Make the single commit `08/T2: performance (code splitting, lazy Monaco, bundle gate, ETags, virtualised views, P10 gates)` with the usual trailers. Nothing else from this section needs redoing.

## W20 decided (2026-10-17)

**Decision.** The human, in the chat on 2026-10-17 (relayed by the orchestrator), decided the grid gate as "Capacity + control" (of options A to D above, C and D together). `perf/budgets.toml` is not touched: threshold 60, `statistic = "min"`, no contract PR.

**Rule implemented** (`web/e2e/perf/grid-5000.spec.ts`, `web/scripts/perf/frame-rule.mjs` with `frame-rule.test.mjs`):

1. **Capacity is the budgeted number.** Five runs of 300 frames in a Chromium launched with `--disable-frame-rate-limit --disable-gpu-vsync`, the same scroll step in every frame; frames a second from the mean frame interval, not capped at 60. `values.min` of `web/grid_5000_rows_scroll_fps` is the minimum of the five and is judged against the floor of 60 by `cargo xtask bench-check`.
2. **Control is an assertion of the spec.** Five 5 s scrolls in the normal, frame-rate-limited Chromium, alternated with five of the hand-written control list in the same browser and execution. The grid's late frames (an interval over 25 ms) must be at most the control list's plus a margin of **8** in 1,500. The margin, and the 60 pairs and 200,000 resamples behind it, are written in the spec: the grid had a late frame in 26 of 60 runs (35 in all) and the control list in 30 of 60 (40 in all); the same code would fail by luck about once in 1,200 executions. Sensitivity, measured by adding a 30 ms hitch to the frame loop: every 4 s gave 13 late frames in five runs, every 2 s 19, every 1 s 27, against an allowance of about 11, so a hitch at least every 2 s fails the gate (passes by chance about 1 in 300) and one every 4 s fails about four times in five. A cost that stays under 16.7 ms a frame (a box-shadow, a layer for every row or cell) gave 1 to 13 late frames and is not seen by this assertion: that is what the capacity number and its floor are for (172.8 fps measured, 2.9 times the floor).
3. **Informational, in the same result file:** `vsyncLocked` (the original W20 metric, min of five 5 s runs of `min(60, 1000 / mean frame interval)`; 58.8 to 59.8 in the 10 validation executions, which is why it is not a gate on this machine), `headroom`, `control` and `controlRule`. The comments in `results.mjs`, the `web-perf` comment in the `Justfile` and the spec say that the number is a capacity and not the display's rate.

**Does it pass reliably.** 11 executions of the new spec on the same machine (Chromium 141.0.7390.37, 4 cores, load average 0.7 to 1.5): 11 passed (10 on their own, 1 inside the `web-perf` steps). Capacity minimum 161.6 to 172.8 fps (floor 60); the grid's late frames 1 to 4 against the control list's 0 to 4, allowance 8 to 12. Then `cargo xtask bench-check` on the five P10 budgets: 5 pass, 0 fail (initial JavaScript 131.0 KB, route change p95 31 ms, diff p95 196.2 ms, grid capacity 172.8 fps, Lighthouse 96).

## T4 Phase A, S0 (shell): result and what needs a human (recorded 2026-10-17, uncommitted, no commit made)

**Status.** S0 and its checkpoint are done in the worktree, uncommitted (the integrator makes the one `08/T4` commit). No blocker for the slices. Nothing in `perf/budgets.toml` was touched.

**Checkpoint, measured** (Chromium 141.0.7390.37, 4 cores, load average 1.3 to 4.2 during the timing specs):

| Gate | Result | Budget |
|---|---|---|
| `pnpm lint`, `pnpm typecheck` | clean | clean |
| whole `vitest run` | 127 files, 2,269 tests passed (the two later edits, `AppLink.test.tsx` and `dialogs.test.tsx`, re-run: 853 passed in the touched directories) | all |
| `pnpm build` + dist scan | initial JavaScript **185.5 KB gzipped** (585 KB raw), one file, dist-check clean | 250 KB; was 133.7 KB (T3 recorded 131.0 KB), so the shell cost 51.8 KB, of which about 33 KB is Radix menu, tooltip, popover and floating-ui and about 13 KB the shell's own code. The palette (cmdk), the switcher list and the shortcuts sheet are chunks of their own and are not in the 185.5 KB. Headroom is 64.5 KB for the slices' shared code that lands in the entry chunk. |
| `chromium` project (existing + new specs) | 217 passed, 0 failed, 0 skipped (with `LK_REACT18_DIR` set, so the visual clip ran) | all |
| route change, cached, p95 | **35.0 ms** (p50 23.2, p99 39.9); T3 recorded 30.1 | 100 ms |
| diff of 6,000 lines, p95 | 182 ms | 500 ms |
| grid capacity, min of five | 182 fps (late-frame rule against the control list passed) | 60 fps |
| `cargo xtask bench-check` on the four measured P10 ids | 4 pass, 0 fail (Lighthouse not run) | |

**New e2e specs.** `e2e/csp/shell.spec.ts` (shell overlays under the production CSP, light and dark: 0 violations), `e2e/keyboard/keymap.spec.ts` (one test per row of the Global, Go to, Command palette and Dialogs scopes, with negative cases, and a last test that fails when the keymap gains a row in those scopes that has no test), `e2e/visual/chrome.spec.ts` (chrome-only tier 1, below). `e2e/perf/splitting.spec.ts` was changed because every screen is now its own chunk and the shell's idle work loads the sidebar destinations the role may open and the palette (never an Admin screen for a Viewer, never the editor); the first visit's scripts total about 199 KB gzipped, under 250.

**Found and fixed while testing.** (1) `DestructiveConfirm`: Enter on the focused verb button confirmed (a button's own Enter is a click), against its own doc and the keymap row "never in DestructiveConfirm"; fixed with a `keydown` guard and a unit test (it failed first). (2) The swimlane switcher's filter field was not focused once its lazy chunk had loaded, so typing right after opening did nothing; fixed with a unit test (it failed first).

**Visual, what ran.** Tier 1 against the rendered reference, chrome only, Shell frames 1a (light) and 1b (dark), 1440x900, threshold 0.2 per pixel, limit 0.5%. Ran, in the same Chromium, with the design's own runtime (`support.js`) and React 18.3.1 from `npm pack` (their SHA-384 equals the SRI in `support.js`), the clock fixed: **sidebar 0.03% (1a) and 0.04% (1b); header 0.019% (1a) and 0.017% (1b)** of pixels differ. The page below the header is not compared (stubs). The reference is rendered with three normalisations, written into `render-references.mjs`: body margin 0 (the board places the frame flush), `button { font: inherit }` (the standalone page leaves buttons in Arial, which the app's reset and the design's host do not), and the notifications bell removed (DEVIATIONS row). The first run before those and before matching the Shell page's data and the breadcrumb's line height was 4.4% to 33%. Tier 2, the app-owned baselines (`__baselines__`) and the other 40-odd frames were not built: baselines are CI-only, and the other frames belong to the slices.

**Needs a human.**
1. **W30 (React 18 for the reference renderer).** The sandbox cannot reach `unpkg.com`, so the visual spec reads the two UMD files from a directory in `LK_REACT18_DIR` (made by `npm pack react@18.3.1 react-dom@18.3.1`, outside the repo, no dependency added). Without it the spec is skipped and says `NOT RUN` with the reason (never a pass). CI needs either the plan's dev dependency (`react18` and `react-dom18`, W30) approved, or that directory provided.
2. **Header drift badge.** The design draws the header's drift badge for `sitb` in the NFS-ahead colours; the app colours it by the most urgent state (conflict first), as for every swimlane. The Shell page's data has only `nfs_ahead`, so the two agree there; a lane with a conflict shows red in the app. Say if the design owner wants one colour in the header.
3. **The 51.8 KB.** The initial JavaScript is inside the budget with 64.5 KB left. It is mostly the Radix overlay primitives the sidebar, header and menus need from the first paint. If a slice's entry-chunk code grows it past about 225 KB (the warn line), the first thing to move out is the tooltip and menu (a lazy account menu).

## T4 Phase C1 (integration of the eight slices), recorded 2026-10-17, uncommitted, no commit made

**Status.** The slices compiled for the first time here. `pnpm typecheck` (both configs, `tsc -b`), `pnpm lint`, the whole `vitest run` (274 files, 5,552 tests, no unhandled error), the generated client (no diff), `pnpm build` and `pnpm build:perf` (bundle gate and dist scan), `pnpm audit`, the chromium Playwright project (298 specs, 0 failed, with `LK_REACT18_DIR` set so the chrome comparison ran) and `cargo fmt --check` pass. `cargo xtask bench-check` on the five P10 ids: 5 pass (initial JavaScript 192.7 KB of 250, route change p95 31.2 ms of 100, diff p95 157.2 ms of 500, grid capacity 187.6 fps of 60, Lighthouse 92 of 90, measured on a quiet machine with Chromium 141.0.7390.37). Phase C2 (the new visual, keyboard, journey, axe and layout specs) and C3 (the commit, the full `web-perf`) are not started.

**Type errors.** 25 diagnostics (17 distinct sites) in 7 files on the first compile: `last_scan_at` is `number | null` in the contract (Findings read it as a number: `latestScan`, "No scan yet"), `exactOptionalPropertyTypes` (`TextSearch`, the new dialog props), the missing `swimlaneKeys.drift`, the page-loader table (one documented cast after `satisfies StrictLoaders`), `RefPicker.test` (`label` widened to string), `useFileEntry.test` (a cast had hidden a wrong `kind` and `role`).

### Applied from the ledger
- **Keys.** `swimlaneKeys.drift`, `docKeys.forFile(file, setting)`, `fileKeys.draft(id)` (a new root, `draft`, for the approved route `/drafts/:id`; the plan's key list predates CG17), `fileKeys.baseline(id, path)`. S2's shim `mocks/design/swimlanes/drift-key.ts` is deleted with its four imports; S3's local builders use the shared factories.
- **Routes.** `compare` reads `lanes, file, only, channel, left, right`. Redirects can now carry search keys (`routes/redirect.ts`): `/docs?q=` goes to `/search?type=docs&q=`, `/grid?file=&swimlanes=` to `/compare?file=&lanes=`, `/swimlanes/:id/restarts` to Services, `/swimlanes/:id/history?path=` to the file with its history panel; `/findings?swimlane=` filters Findings (the screen's `lanes` wins). `routes/links_resolve.test.tsx` reads `LinkTemplates::default()` from the Rust source and fails on a template without a place. **Still unrouted (show Not found, listed in that test with the reason):** `/swimlanes/:id/effective` and `/served` (CG8, T6) and `/swimlanes/:id/pull-requests` (T5).
- **Permission screens** (Approvals, Audit, Admin, through `RouteGate`) carry `RoleChangeAction` (lazy, nothing for a person with no role).
- **Dialogs.** The shared `AlertDialogContent` and `DialogContent` remember the control that had focus when they opened and give it back (or `returnFocusRef`); S1's two workarounds and S8's `features/admin/focus.ts` are gone (People keeps a small inline refocus for a write that applies at once and disables its control, which is not a dialog). `ConsequenceDialog` has `initialFocus="first-field"` (the design's rule 8b for safe dialogs; set on S8's six form dialogs and the doc upload); `DestructiveConfirm` has `children`, `canConfirm`, `titleIcon`, `actionIcon`, `showMatchHint`, and S3's `DeleteDialog` is built on it; titles are drawn through `SafeText`; `ProblemDialog` has one Close (no corner cross unless asked).
- **Components.** `PersonalTokens` `heading` ("Your tokens" on both screens); `Banner` variant `locked` (S2's `LockBanner` uses it); `DataTable`: `rowClassName`, selection colour from `aria-selected` (wins over the cursor tint), `onOpenInNewTab` (Control or Command and Enter), `x` as well as Space with `{ extend }` for Shift, `rowHeight`, `headerHeight`, `onRowClick`, `moreRows`, and `aria-rowcount` of -1 while the hub has more pages (VirtualList `totalRows`). Adopted: S2 (new tab, tinted changed row with the design's `--info-bg` and 1 px `--info-border`), S5 (muted channel rows), S6a (`onRowClick`, new tab to the file at its first line, the selected-row class hack removed), S7 (`QueueTable` and `AuditTable` are now DataTables with `rowHeight` 48; `approvals/cursor.ts` is deleted).
- **Navigation.** `useNavigateTo` skips a push to the address the history is already at and is memoised; `useReplaceTo` is shared and the four copies (approvals, findings, swimlanes, compare) and S3's `useGoTo` are deleted; `openInNewTab(href)` opens only a path of the app with `noopener noreferrer`.
- **Badges.** Approvals shows `25+` when the hub has more pages. **Admin** (the human's later decision, treated as required): the Admin item counts the access requests that wait (`badge: "admin"`, `usePendingRequests({ enabled, autoLoad })`, one cache entry with the Users section, live on `access_request_created`; "Admin, 3 pending requests" / "1 pending request"; nothing for anyone but an Admin, and no request is made for them). Initial JavaScript 191.2 to 192.7 KB for it.
- **One owner for "Live updates paused":** the shell's banner. `QueryBoundary` speaks only for `age` and `refetch-failed`; S5's `ownsStaleBanner` and S8's `AdminBoundary` (26 call sites) are deleted. Their tests that asserted the second banner were rewritten to assert the absence of it (a removed feature).
- **Agent mapping:** S5's copy in `StatusPage` uses `agentView`. **Finding codes:** S2's `FINDING_CODE` table is gone (`codeOf` of the Findings screen).
- **Sign-in h1** is "Lanekeeper" (the Access Screen's own); `session.test`, `AppShell.test`, `csp.spec` and `live.spec` said "Sign in". `session.test` for the Admin screen says h1 "Admin".
- **Audit Integrity (the human's recommendation):** the "all clear" strip and its Details are dropped; a panel exists only when two consecutive loaded events do not link, says so, and says it is not the hub's own verification. The chain hash display and copy in the drawer stay.

### Cross-slice failures
- `server-state.test.ts` and `api-types.guard.test.ts` pass **without being changed**. The S3 tables are keyed by the contract's enums (`ProposalStatus`, `Confidence`, `PrJobState`) or are a function (`titleOf`); S4's two were already gone; two local type names that equalled schema names were renamed (`AuditDiffLine`, `HitFileKind`).
- **`useMutation` result in a dependency array:** none left (S3's `PrDialog` was the only one; a scan of every hook-result in every dependency array found 11 other uses, all stable). `lib/query/mutation-deps.test.ts` now fails on one.
- **`EnvironmentTeardownError` (reproduced once in a wide run, from `SearchDocs.test.tsx`).** Cause: jsdom has no `requestIdleCallback`, so `useIdlePalette` fell back to a 1.5 s timer that imported the palette chunk (and its admin endpoints) in the middle of a test, still loading when the file ended. Fix: `src/test/setup.ts` gives the browser an idle callback that never runs (a test of idle work installs its own); two tests (`setup.test.ts`, `AppShell.test.tsx`) fail without it. The whole suite then ran twice with no unhandled error.
- **AppShell "marks the item as the current page" (once, wide run).** Cause: the real file screen is a chunk of hundreds of modules; under load its first import took longer than Testing Library's default 1 s wait. Reproduced 3 of 3 with the 1 s wait while another vitest run was going, 0 of 3 with the wait at 5 s. Fix: `asyncUtilTimeout` 5 s and `testTimeout` 30 s for every jsdom test (`setup.ts`, `vite.config.ts`), and the shell tests use a stand-in file screen. `session-expiry-dirty.test` likewise uses a stand-in (the real file screen has no h1 "Files").

### Dropped, deferred, left for a human
- **Dropped (made untrue):** S2 DEVIATIONS row 5 (no selected-row background, the live change only in a cell, "no page banner for an offline agent (O01)": the first two exist now; O01 is the Swimlanes frame with an offline row, which the app draws). The S7 Integrity row was rewritten, the S6a "Findings: rows" row and the S8 Users row were reworded (`DataTable` can do what they say it could not).
- **Left for T5:** three different `SeverityBadge`s (swimlane Findings tab, Findings screen, file history; S2's and S6a's disagree on colours) and S2's finding titles against S6a's names; the shared W32 token-expired flag (S3's is page-local); `Live.onEvent`; a `finding` search key to deep-link the drawer; S4's 3-line lane header (`headerHeight` exists now).
- **Needs a human:** (1) `e2e/perf/splitting.spec.ts` asserted that ALL scripts of a first visit, idle prefetch included, are under 250 KB gzipped; with real screens that is 274.5 KB (initial JavaScript, the P10 id, is 191.2 KB). I split it: the first visit with idle held back (shell, one screen, live engine) is held to 250 KB; the idle loads are asserted by name (only the role's sidebar screens and the palette, no editor, no Admin screen for a Viewer) and their total is reported as an annotation, **not budgeted**. Say if the idle total should have a number. (2) `e2e/keyboard/keymap.spec.ts`: `/` on the Swimlanes screen focuses the screen's filter (a screen's row beats the shell's); the shell's "go to Search" is tested from Approvals, and the filter case is asserted too. (3) Lighthouse on the lab pages is 92 (was 96): above the floor of 90, close to it; Phase C3 retargets it to the real routes. (4) S5: a rendered doc may contain its own `<h1>`, so the reader page may have two; the axe run in C2 will say. (5) The copy buttons for the join token and the MCP token are a separate agent's.

## T4 Phase C1b (the copy-affordance UX pass) and decision 11, recorded 2026-10-17, uncommitted, no commit made

### Decision 11: a copy button for one-time secrets, and a consistent copy experience (human, 2026-10-17, relayed by the orchestrator)

**The request.** "Go out of the way to make subtle UX experience like copy buttons." The human wants the one-time secrets (the admin join token and the personal MCP token made on the MCP page) to have a copy button, and a polished, consistent, unobtrusive copy experience across the whole app. This overrides the earlier choice of S1 and S8 (no copy button for a secret, because the shared `CopyButton` said never to copy one).

**What was built** (all in `web/`, test first; the rest of the list is in the table below).
- `CopyButton` (generic, not for a secret) rewritten: an icon button named for what it copies ("Copy request ID"), tooltip "Copy", a bubble with a check and "Copied" for 1.5 s (no toast), the same words said through an always-present visually hidden `role="status"`, a second click is announced again, focus is never moved and the button stays enabled, the name does not change, nothing shifts (the icons share one box, the bubble is in a portal, a labelled button lays out both words), a 24 px target that takes the 18 px of the design's button in the line, tokens only (light and dark), `animate-lk-pop-in` for the check (off under `prefers-reduced-motion` in `base.css`). When the clipboard is absent or refuses: "Couldn't copy. Select the text and copy it manually." in the bubble and the text of `source` selected (an input is focused and selected, so Control+C works). A `secret` prop is a type error and renders nothing. `what` is required. A single-line value with a line break or a control character is not copied ("Not copied: ..."); a value with a bidi or zero-width character is copied and the confirmation says so (S8). `multiline` is for text (the session-ended draft).
- `SecretCopyButton` and `SecretField` (`components/app/secret/`): a labelled "Copy token" / "Copy join token" button that writes only inside a click (and only when `navigator.userActivation` is not saying "inactive"), reads the value through `getValue()` at that moment and keeps it nowhere (not state, a key, an attribute, a title, the name, the words), says "Copied. It will not be shown again." with no value, never logs, is not a toast, goes with the secret, clears its timer on unmount, and refuses an empty value, a redaction marker or a control character. `SecretField` is the read-only mono field: focus, a click and a double click select the whole token. A lint rule (`lint/restricted-imports.mjs`, `SECRET_DISPLAYS`) lets only `PersonalTokens` and `JoinTokenCard` import the folder; `secret-imports.test.ts` reads the source tree and says the same, and that neither display imports the generic button.
- `CopyValue` ("value [copy]": the button shows on hover and on focus within, is always in the keyboard order, always shown where there is no hover, and stays while it says Copied), `CopyLink` + `absoluteLink` (a permalink made from `linkTo` and this page's origin, refused for anything that is not an absolute path of the app; a fragment is set through `URL`), `supportDetails` (the details for support), `RequestId` with `details`.
- Wired (12 offers, below): the two secrets, the details for support, the file link, the swimlane name, the proposal number and link, the PR address, the doc link and section link, the audit request ID and previous hash, the finding path and base file, the compare drawer's setting path, the status note's setting path and value.
- The platform check (`e2e/csp/clipboard.spec.ts`, 7 tests, real `web_fixture` header): see "Permissions-Policy" below. New mock scenario `design-access-mcp-token-issued` (the existing ones answered no POST).

**Residual risk** (also `docs/threat-model.md`, "Copying a one-time secret, and copying in general"). Another application, or a clipboard-history manager, can read the token after the write, and a web page cannot take it back out (writing an empty string later needs the document focused and could overwrite something else the person copied). The token is shown once, is revocable on the MCP page (a join token expires in 24 h and works for one agent), the button is click-only, keeps nothing in app state, never logs, and the person is told it will not be shown again. Copying text from NFS and people (paths, names, values) has its own risk, a pasted line break or a hidden character; the checks above are the mitigation, and they are the reason a value with a line break is refused rather than copied.

**Out of scope for this PR, a separate future feature (not built, not stubbed).** Admins listing and revoking OTHER people's MCP tokens. It needs a contract operation (`api/openapi.yaml` has `listMcpTokens` and `revokeMcpToken` for the signed-in person's own tokens only), so it is a contract-change PR first.

### Inventory: where a person plausibly wants to copy something, and what was decided

"Dense table or tree row" means a `DataTable`, `TreeView` or virtual-list row: the keyboard cursor is `aria-activedescendant` on the grid, so a button in each row would add a tab stop per row (up to 500) and make the table longer to cross; those rows get no inline copy, and the drawer or the file view that the row opens carries it.

| # | Place | What | Decision | How, or why not |
|---|---|---|---|---|
| 1 | Every error: `ErrorState`, `QueryBoundary`, `WriteError`, `ProblemDialog`, the token outcomes, the PR job steps, the search errors, 403, 404 and error pages, the doc reader's 413, the Activity feed | request ID | Yes (existed) | `RequestId`, now the polished button. Design: "errors always show a copyable request ID". |
| 2 | An error that came with a response | request ID, problem title and type, status, time | Yes (new) | "Copy details for support" after the request ID in `ErrorState`, only when there is a response (status over 0). Built from the `ApiProblem` alone: never the request body, never `detail`, one line per fact. `WriteError` and `ProblemDialog` hold only a request ID string, so they keep the plain button. |
| 3 | File header | path | Yes (existed) | `PathChip`. |
| 4 | File header, history rows, save/conflict/draft/apply dialogs, proposal facts, compare file panel | content hash | Yes (existed) | `HashChip`, `FilePanel`. |
| 5 | File view | link to this file, or to this line | Yes (new) | `CopyLink` in the file header, built with `linkTo("files", ...)`; "Copy link to line N" when a line is chosen. Tab, panel and diff side are not in a permalink. |
| 6 | File view | a line of text, a diff hunk | No | Text is selectable; the code view and the diff are drawn from server hunks, a line is not a value, and a per-line button is a tab stop per line. The line link (row 5) is the shareable form. |
| 7 | File and swimlane trees | path | No | Tree rows (dense, cursor). The file header has the path. |
| 8 | Session-ended alert | the person's unsaved draft | Yes (existed) | `CopyButton multiline`; line breaks allowed, other control characters refused. |
| 9 | Destructive confirm | the name to type | Yes (existed) | `CopyButton`. |
| 10 | Swimlane list rows, tabs (Drift, Findings, Services, Activity) | swimlane name, rows | No | Dense table rows. |
| 11 | Swimlane detail header | swimlane name | Yes (new) | A button beside the h1 (not in it, so the title keeps its name), shown on hover and focus. |
| 12 | Search: text hits, settings table, docs hits | path, line, setting path, value | No | Tree rows, `DataTable` rows. A hit is a link that opens the file at the line, where the header has the path and the link. |
| 13 | Findings table | path, code | No | Dense table rows. |
| 14 | Finding drawer | file path, base file | Yes (new) | `CopyValue`; the drawer opens with focus on Close (a button that appears on hover is never where a drawer opens; a test says so). |
| 15 | A finding as a link | permalink | No | The route cannot name a finding (the `finding` search key is a T5 item), so a link would not open the drawer. |
| 16 | Compare grid cells, values in the diff | value | No | A cell click is the compare gesture; a value is in the diff text, and a hidden value has no clipboard text. |
| 17 | Compare drawer | setting path | Yes (new) | `CopyValue`. |
| 18 | Feature status table rows | setting, flag | No | Dense table rows. |
| 19 | Feature status note (`?setting=&lane=`) | setting path, value | Yes (new) | `CopyValue` for the path; for the value only when it is not the redaction marker (`isMaskedValue`). |
| 20 | Docs reader | link to the doc, link to the section | Yes (new) | `CopyLink` in the toolbar; "link to this section" appears when a heading is chosen (fragment through `URL`). The table of contents has no per-heading button (a tab stop per heading). The doc's path is not drawn, so the link is its copy. |
| 21 | Approvals queue rows | proposal, path | No | Dense table rows. |
| 22 | Proposal drawer | proposal number "AP-2046", link to the proposal, paths, hashes | Yes (number and link new; chips existed) | `ProposalNumber`: two buttons shown on hover and focus of the number. |
| 23 | Audit table rows | event, request ID | No | Dense table rows. |
| 24 | Audit event drawer | request ID, previous hash, chain hash | Yes (ID and previous hash new, chain hash existed) | `CopyValue`; the chain hash keeps the design's labelled footer button (one "Copy chain hash"). |
| 25 | PR dialog | PR address | Yes (new) | A button beside the link, shown on hover and focus; the same address the link opens (https, checked before it is a link). |
| 26 | PR dialog | base branch | No (unchanged) | `BranchChip copy={false}`, a fixed fact the person did not choose. |
| 27 | Admin: join token (shown once) | the token | Yes (new, the human's request) | `SecretCopyButton`. |
| 28 | Personal MCP token (shown once; MCP page, `/me/tokens`, Admin MCP section) | the token | Yes (new, the human's request) | `SecretCopyButton`. |
| 29 | Personal MCP token list | label, id | No | Nothing there is useful to paste; the token itself cannot be shown again. |
| 30 | My GitHub token page | the PAT the person types | Never | It is input, not output; there is nothing to copy back and a copy button would invite pasting it elsewhere. |
| 31 | A revealed setting value, a masked value | the value | Never | M1 and the plan: no copy of a revealed value; a masked value has no clipboard text. |
| 32 | Admin: users (email, GitHub login), agent service account, tenants, rules | text | No | Dense table rows, or an input the person edits. |
| 33 | Toasts, the palette, the switcher, the shortcuts sheet | anything | No | A toast holds nothing worth copying and disappears; the others are navigation. |
| 34 | Times | a timestamp | No | The text is selectable; a time has no use outside the screen. |
| 35 | "curl-free API calls" | | None | The app builds no such thing. |

35 rows: 17 are "Yes" (12 new offers: rows 2, 5, 11, 14, 17, 19, 20, 22, 24, 25, 27, 28; and 5 that already existed and were kept and polished: rows 1, 3, 4, 8, 9), 15 are "No" with the reason above, 2 are "Never" (rows 30, 31) and 1 is "None" (row 35).

### Permissions-Policy (the platform check)

`crates/hub/src/headers.rs` sends `accelerometer=(), autoplay=(), bluetooth=(), camera=(), display-capture=(), encrypted-media=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), publickey-credentials-get=(), screen-wake-lock=(), serial=(), usb=(), xr-spatial-tracking=()`. It does not name `clipboard-write`, and the default allowlist of that feature is `self`. Measured in Chromium 141.0.7390.37 against the real header (the harness proxies the page to the Rust `web_fixture`), `e2e/csp/clipboard.spec.ts`:
- `document.permissionsPolicy ?? document.featurePolicy` says `clipboard-write` is allowed (true) and `camera` is not (false), so the check says something.
- On `/me/tokens` the secret copy button writes the token (read back with `navigator.clipboard.readText()` and compared with the field), says "Copied. It will not be shown again.", the button and the field do not move by one pixel, focus stays on the button, no toast, no word of the token outside the field, and nothing is left after "I have stored it". The keyboard path (Tab to the button, Enter) works. A request ID on the 403 page copies and shows "Copied" in the bubble. The CSP watcher is empty in every test.
- Control (the same page with `clipboard-write=()` added to the header by a route handler): the browser refuses, the button says "Couldn't copy. Select the text and copy it manually." and the token in the field is selected. So **the header does not block it and no change in `crates/hub` is needed**; if a deployment ever adds `clipboard-write=()`, the web side degrades as above.
- Without a Clipboard API at all (an insecure page), and with a `writeText` that rejects, the same message and the same selection.

### What changed in existing tests (nothing loosened)

- `feedback.test.tsx`: the two `CopyButton` tests (name becomes "Copied"; the live region says "Copy failed") describe the old button; they are replaced by `CopyButton.test.tsx` (21 tests: name, tooltip, 24 px, Copied, announcement, repeat announcement, focus, labelled button, Escape, failure with and without a clipboard, input selection, control characters, hidden characters, empty, `secret`, motion, tokens). `RequestId` keeps its test and gains the details test.
- `admin-rules.test.ts`: "never imports the copy button" becomes: the generic button is never imported in the Admin area, and `SecretCopyButton` appears only in `JoinTokenCard`.
- `SwimlanesAdminPage.test.tsx` (join token) and `PersonalTokens.test.tsx`: "no copy button" becomes "a copy button copies the token once clicked, says it will not be shown again, leaves nothing in any cache, the console or the toaster, and is gone when the token is dismissed".
- `FindingsPage.test.tsx` (Viewer): "no button except Close" is now "no button except Close and the copy buttons", which is what the test meant (nothing to disable); a new test says the drawer still opens with focus on Close (it did not, once the path had a copy button, until `onOpenAutoFocus` was set).
- `ProblemDialog` (409, 423, sent for approval) now opens with focus on the dialog itself (`dialogs.test.tsx`, new test; DEVIATIONS row). Found by the full chromium run, not by the unit tests: with the request ID's copy button as the first control, focus landed on it, its hint tooltip opened, and the first Escape closed the tooltip instead of the dialog (6 failures in `e2e/csp/overlays.spec.ts`, 3 dialogs in 2 themes). The first control was the copy button before this pass too, but the old button had a native `title` and no layer to take the Escape.
- The session-ended draft is several lines, which the new single-line rule would have refused; the draft copy is `multiline`, and a new test in `session-expiry-dirty.test.tsx` copies three lines whole.

### Found, for the owners of other work

- **axe `region` on an open tooltip (pre-existing, shared).** Radix renders a visually hidden `role="tooltip"` copy of every tooltip into a portal under `<body>`, outside every landmark, and axe's best-practice `region` rule names it whenever any tooltip is open (the shell's icon tooltips included). The "Copied" bubble now draws its words `aria-hidden` and gives the label to Radix, which removed the visible text from the finding; the hidden copy remains. A scratch axe run (wcag2a, 2aa, 21aa, 22aa, best-practice, `target-size` on) of the 403 page, the MCP page with the token shown and copied, and the file view with a copied path, in light and dark, found no `target-size` and no contrast finding; the only finding is that one `region` node on the pages where a tooltip was open. Phase C2's axe run will meet it for any state with a tooltip open; the fix is in the shared `TooltipContent` (a portal container inside a landmark), not in the copy components.
- **The ⌘⇧C hint** of the Style Guide's tooltip is not shown: there is no such keymap row (no new rows, H-11).
- **Entry chunk:** 192.7 to 194.0 KB gzipped (+1.3 KB; the limit for this pass was about +2 KB). Part of it is `ErrorState` and `RequestId`, which are in the entry; the rest of the copy code is in the route chunks that use it.

### Checkpoint, measured (Chromium 141.0.7390.37, 4 cores; final code, after the last edit)

| Gate | Result | Before |
|---|---|---|
| `pnpm lint`, `pnpm typecheck` | clean, clean | clean |
| whole `vitest run` | 282 files, **5,718 tests** passed | 5,552 |
| `pnpm build` (bundle gate and dist scan) | initial JavaScript **194.0 KB** gzipped of 250 (616 KB raw), dist-check clean | 192.7 KB: **+1.3 KB** (limit for this pass: about +2 KB) |
| `pnpm build:perf` | dist-check clean (194.7 KB) | |
| `pnpm audit` | no known vulnerabilities | |
| chromium project, `--workers=2`, with `LK_REACT18_DIR` (the chrome comparison ran) | **308 passed**, 0 failed, 0 skipped, no NOT RUN | 298 (7 of the difference are `clipboard.spec.ts`; `assets.spec.ts` has one test for each built chunk, and the build has three more) |
| `cargo fmt --check` | clean | |
| New dependency | none (`package.json` and the lockfile are unchanged) | |

Process notes. Run red first: `CopyButton` (19 of 21 tests failed on the old button), the secret components (no module), the lint rule (the rule removed turns its test red), the details for support, and the `ProblemDialog` focus. Written before the code but first run after it: `CopyValue`, `CopyLink`, `supportDetails`, `checkCopyable` and the wiring tests; for the 13 wired places (the two secret displays, the details, the file link, the swimlane name, the proposal number and link, the PR address, the doc links, the audit rows, the finding path, the compare path, the status note, the draft) each wiring was then removed once and its test went red (a scratch script, not kept), and restored. The first full chromium run (before the last fix) found the `ProblemDialog` Escape regression above; the second run is the one in the table.

## T4 summary (Phase C3, the one `08/T4` commit), recorded 2026-10-18

**Status.** T4 is built and committed once, with every gate named below green on the machine named below, except the ones listed under "Could not run". Nothing in `perf/budgets.toml` or its thresholds was touched. No dependency was added (`package.json` and the lockfile are unchanged).

### Built, per slice (W27)

| Slice | What it delivers |
|---|---|
| S0 shell | Frame, sidebar (role-gated, with the Findings, Approvals and Admin badges), header with breadcrumb, swimlane switcher, command palette, shortcuts sheet, live indicator and the "Live updates paused" banner (one owner), skip link, focus rules, keymap (every Global, Go to, Palette and Dialogs row tested). |
| S1 access | Sign-in, onboarding, access requested, no access, expired session, the Access Screen frames (13 scenarios). |
| S2 swimlanes | The list (40 rows, filters, live "changed" rows, sync windows), the detail with its tabs (Overview, Drift, Findings, Services, Activity, Audit), restart and notify with approval and lock states. |
| S3 file view | Head, tabs (NFS, Git, Baseline, Diff), tree, content with masks, diff (unified and side by side, 6,000 lines), edit with Monaco (tag-aware), save, convert line endings, upload, delete, revert, history, docs panel, PR dialog and job, every conflict, lock and sent-for-approval answer. |
| S4 compare | The grid (5,000 rows by 40 swimlanes, cursor pages, search, channel filter, pick two cells, cell drawer), whole-swimlane compare (refs, unified diff, binary, nothing differs), the swimlane and file pickers. |
| S5 search, docs, status | Text, settings and docs search; the doc reader (sanitised, headings, links); feature status. |
| S6a findings | The findings list and drawer (all kinds), live and held-still lists, offline and truncated states. **Adoption wizard (S6b): BLOCKED, gated on CG6** (no operation in `api/openapi.yaml` for it). |
| S7 approvals, audit | Approvals queue and drawer (approve, reject, own proposal locked), the audit table and event drawer, the integrity panel (shown only when two loaded events do not link). |
| S8 admin | Users and access requests, swimlanes (manage, tenants, join token shown once), path rules, merge rules, tiers, folders, notifications (webhook masked), MCP tokens (own only). Four T5 sections say "Not available yet". |
| C1b | Copy affordances across the app (decision 11): 17 offers of 35 places inventoried, `SecretCopyButton` only in the two secret displays. |

Decision 10 (the server-state layer) holds in every slice: keys from the factories in `lib/query/keys.ts`, writes through `lib/query/invalidate.ts`, optimistic updates only through `lib/query/optimistic.ts` (no slice uses it; the contract has one honest candidate and it stays that way), no server data in client state. Decision 11 is C1b above. The Admin sidebar badge (the human's later decision) is live for an Admin only.

### What this last round fixed (ledger items), test first

- (a) Compare diff drawer no longer covers the pick bar's Hide and Clear (the drawer is positioned in the box under the toolbar, pick bar and banners). (b) "Load more" lost during the first-connect refetch: `loadMore` (`lib/query/cursor-pages.ts`) now waits for the query to be idle and asks again, at most three times; the grid's refresh waits for an in-flight next page. (c) The approvals queue cursor survives closing the drawer (history state, `features/approvals/restore.ts`). (d) The split handle's Home and End collapse and expand (the tree panel is collapsible and inert when collapsed). (e) `diff.next-previous-change` has its real owner in `keyboard/bindings.ts`. (f) Design mocks agree (file-diff hunks and text, `consistency.test.ts`) and satisfy the OpenAPI validator. **The ledger's claim "restart-approval should answer 202" was wrong: the API answers 200 for a restart sent for approval, so the journey was fixed and the mock kept.** (g) The skip link is the first Tab stop on a first load; the h1 takes focus only after in-app navigation (`shell/focus.ts`); `journeys/admin.spec.ts` and `a11y/rows.spec.ts` follow. (h) `keyboard/file.spec.ts:159`: the dirty check read a stale render closure; `useEditSession` keeps the latest session in a ref (`isDirty`). (i) Type-ahead against `f` and `g`: the handoff keymap wins, DEVIATIONS row. `LivePaused.test.tsx` timeouts: the tests raced the lazy chunks; a `beforeAll` primes the imports.
- **Compare toolbar (found by the layout specs, deterministic at 1024 px with labels +40%):** the count grew from "50+" to "550+" when the next page arrived and the bar wrapped to a third line, pushing the grid down 24 px. The count's box is now 14 rem wide whatever it says (`GridToolbar`), and `e2e/layout/steady.spec.ts` fails without it.
- **Lighthouse on the real file diff route measured 88 (budget 90), not on the lab page it replaced.** Causes, in order of effect: (1) a layout shift of 0.10: the "Large file" banner arrived above the header and pushed the whole page down 42 px. The banners the file brings (withheld, large) are now under the header, where the handoff has them (File frame), and the body is a tree of its own for the placeholder and for the file (CLS 0.002; `steady.spec.ts` and a `FilePage` test). (2) The first load of the file view carried the code of every dialog (about 90 KB raw, in about 25 of the 40 requests of the file route's first wave): all dialogs are now one lazy chunk (`FileDialogs.tsx`), fetched when the browser is idle or when the first dialog is asked for (`dialogs-chunk.test.ts` reads the source). Rejected: Rollup's `experimentalMinChunkSize` raised the page by two points but merged small screen chunks into others, which breaks `splitting.spec.ts` ("a screen is its own chunk", "no screen outside the sidebar is fetched when idle").

### Shared configuration

`snapshotPathTemplate` puts the app-owned baselines in `e2e/visual/__baselines__/`, which `web/.gitignore` ignores with `test-results/`, `playwright-report/`, `.refs/`, `/dist-*/` and `*-snapshots/`; no PNG, report, private dist or snapshot folder can be committed. A missing baseline is created only under `LK_UPDATE_BASELINES` (and then fails the test that needed it); otherwise it is a failure in CI and `NOT RUN` elsewhere. `chrome.spec.ts` fails closed in CI like the others. **W30 (decided by the agent, to confirm):** `.github/workflows/web-e2e.yml` fetches `react@18.3.1` and `react-dom@18.3.1` from the npm registry with `npm pack`, checks the tarballs against pinned sha512 integrity values and the test checks the UMD files against the SRI in the design's `support.js`; no dependency is added. A `visual-baselines` job (by hand) makes the baselines on the runner that compares them. **Not verifiable here:** the commit SHA pinned for `actions/upload-artifact` (v4.6.2) was written from memory; the sandbox cannot reach GitHub to check it, so a human must confirm the pin before the first run.

### DEVIATIONS

140 rows, all `pending`, grouped by screen: the ledger entries V1, V2, AL, KJ and C1b were merged (made-untrue and duplicate rows dropped), and the counts row of Compare now says the 14 rem box. `scripts/deviations.test.mjs` fails on a row with an empty cell, a screen named twice, or a visual-spec exclusion without a row.

### Measurements (Chromium 141.0.7390.37, 4 cores, no other work running)

| Gate | Result | Budget |
|---|---|---|
| `pnpm lint`, `pnpm typecheck` | clean | clean |
| whole `vitest run` | 291 files, **5,914 tests** passed | all |
| `pnpm build` | initial JavaScript **194.5 KB** gzipped (617 KB raw), dist scan clean | 250 KB |
| chromium project (official dist, `LK_REACT18_DIR` set) | **1,624 passed**, 0 failed, **177 skipped as NOT RUN** (the app-owned baselines, which exist only in CI; every tier-1 frame against the rendered handoff ran: V1 76, V2 18, the shell chrome 4) | all |
| route change, cached, p95 (50 samples) | **92.8 ms** (click 96.8, keyboard 74.9; cold 56.1 not budgeted) | 100 ms |
| diff of 6,000 lines, p95 | **129.5 ms** | 500 ms |
| grid, capacity (min of five) | **191.3 fps**, 3.2 times the floor; late frames: grid 6, control list 0, allowed 8 | 60 fps |
| Lighthouse (median of three, lowest page) | **93**: Compare 94, file diff 93, Swimlanes 95 (LCP 1.25 to 1.42 s, TBT 18 to 93 ms, CLS 0) | 90 |
| `cargo xtask bench-check`, the five P10 ids | 5 pass, 0 fail | |
| `cargo fmt --all -- --check`; `cargo clippy -p hub --example web_fixture -- -D warnings`; `cargo test -p xtask --test budgets_registry` (8 tests); `pnpm audit` | clean; clean; pass; no known vulnerabilities | |

**Per-slice unit counts at the end of each slice:** S1 148, S2 254, S3 547 in `features/file` (1,798 with the mocks it validates), S4 188, S5 276, S6a 84, S7 154, S8 208. **End to end:** visual 140 (V1) and 160 (V2), accessibility and layout 1,093 (axe 1,032 scans, 0 violations), keyboard 46, journeys 45, clipboard 7.

**The route-change figure has little room, and a human should know.** The gate run measured 92.8 ms, but five more runs of the same spec, one after another on the same machine, gave p95 of 130.1, 92.0, 79.3, 71.4 and 79.2 ms. The slowest screen is Swimlanes (40 compact rows, 25 in view, about 1.5 ms per visible row: p50 66 to 97 ms, against 24 to 40 ms for Compare, Findings and Approvals; with a window of 450 px it is 45 ms). A render of 45 elements a row is the cost; the options are a lighter row (fewer wrappers, fewer elements), drawing the rows below the first screen in the next frame, or a different budget. I did not choose: the first changes the design's cells, the second gives a faster number for the same work. The budget in `perf/budgets.toml` is unchanged.

### Could not run, and why

- The app-owned visual baselines (177 tests) are made in CI only (`visual-baselines` job). Locally they were generated into a throwaway folder and compared twice (see the result in the commit message); the folder was deleted.
- `just verify` as a whole (the Rust workspace tests, `cargo deny`, `cargo audit`, osv-scanner) was not run, as asked: T4 touches no Rust. Its web part (install, lint, typecheck, `vitest run`, build) is the first part of `verify-08`, which passed.

### Open human decisions (defaults taken are in the code and in DEVIATIONS)

1. The route-change margin above. 2. W30 (React 18 UMD in CI as a workflow step, and the `upload-artifact` pin). 3. `splitting.spec.ts` reports the idle prefetch total and does not budget it; say if it needs a number. 4. Dialog dim over the sidebar and top bar (Shell frames dim everything; the File View frame dims the content only; kept content-only). 5. Banner height 42 px (target size) against the design's 40. 6. Audit Time column: ISO day in 148 px against "Oct 9" in 104 px. 7. Wrapping toolbars (min 40 px) against the design's fixed bar. 8. A copied value with a bidi or zero-width character is copied and says so. 9. The header's drift badge colours by the most urgent state, the design draws one colour. 10. Admins listing and revoking other people's MCP tokens needs a contract operation. 11. CG6 (adoption wizard), CG8 (`/swimlanes/:id/effective` and `/served` still show Not found) and T5 (`pull-requests`, three `SeverityBadge`s, the shared token-expired flag, a `finding` key in the search for a link to the drawer, the 3-line lane header of Compare, G03).

## T5 stage 1 (integration of the four slices), recorded 2026-10-18, uncommitted, no commit made

**Status.** The four T5 slices (attribution and severity, PR links and job progress, masking and reveal, the four admin sections) compiled for the first time here and are wired into the T4 screens. Nothing is BLOCKED. No commit, no push; the stage-2 agents write the new T5 visual, keyboard, journey, a11y and layout specs, and the final stage makes the one `08/T5` commit. No dependency was added (`package.json` and the lockfile are unchanged), nothing in `perf/budgets.toml` or the contract was touched.

### First compile
- `pnpm install --frozen-lockfile`: up to date. `pnpm lint` on the slices as written: clean. `tsc -b`: **8 distinct diagnostics (14 lines, because the app and test configs both report a file)**. Six were the unrouted page (`swimlane-pull-requests` is not in the route table: `DuplicatePr` x2, `PrJobView` x2, `PullRequestsPage`, and its app test's loader); one was `exactOptionalPropertyTypes` (`EventWho` passing `className: string | undefined` to `UnknownAuthorBadge`, whose prop is now `className?: string | undefined`); one was a cast in `mocks/design/admin-t5/world.ts` (`as unknown as RetentionInput`, as the line above it already did for `SeverityRuleInput`). After the wiring, lint found three more of my own (two unnecessary optional chains in a new test, one unused import) and they are fixed.
- The route shim the PR tests carried (`withPullRequestsRoute`, which pushed the route into `ROUTES` when it was absent) is deleted now that the table has the row; the app test uses the real page loader.

### The security finding: S3's masked view failed open (confirmed, then fixed test first)
`features/file/view/MaskedCodeView.tsx` and `masks.ts` dropped a mask whose span was not a marker (or named a line the text does not have) and drew the raw text. Four tests were written in `ContentView.test.tsx` ("masks that cannot be trusted never draw the raw text") BEFORE the view was replaced. **Red against S3's view: 3 of 4 failed with the planted value in the DOM** (a raw value under a mask: `1a: 12k: hunter2-correct-horse3b: 2CR...`; a mask naming a line the text does not have; a span off by one byte that holds part of a marker and the value; the fourth, "still shows the other lines", passed and now also asserts "Hidden line"). **Green after the swap** to `MaskedTextView` and `RevealableMaskedText` (fail closed: the line is hidden, or the text refused), 20 of 20 in the file, plus a case with the Reveal control present (no Reveal button for a hidden line, the value in no part of the DOM). S3's `MaskedCodeView.tsx`, `masks.ts` and their two tests are deleted; `components/masking/plan.test.ts` (74 tests with a property test) covers every case S3's `masks.test.ts` did, with the opposite, fail-closed, rule.

### Applied from the ledger (each checked against the code first), test first
- **Mocks.** `mocks/design/index.ts` lists `attribution`, `prs`, `masking` and `admin-t5` (`design.test.ts` and the OpenAPI check of `mocks.test.ts` pass: 2,246 mock checks).
- **B (pull requests).** W1 to W7: the route `swimlane-pull-requests` at `/swimlanes/:id/pull-requests` (the hub's `LinkTemplates::default().pr`, read from `crates/hub-mcp/src/output/links.rs`; not `/pull-requests`), Viewer, search key `state`; page loader; `inventory.test` approved addition; `links_resolve.test` (`LATER.pr` deleted, `pr` expected to land on the page); crumbs; the swimlane switcher keeps the tab. W8: ONE `usePrBadges(lane)` per screen in the file header (`FileHeader` has `prBadge` and `changedBy` slots), the Drift tab, the swimlane Findings tab and the global finding drawer (a drift finding with a file only; the cross-swimlane table has no badge and makes no request; the drawer's read is `enabled` only for such a finding); each has a test that counts two requests (open, merged) however many rows there are. W9: `PrDialog` uses `duplicatePrOf`, `DuplicatePr` + `OpenPrButton`, and `PrJobProgress` (the job view kept fresh by `pr_job_progress`); S3's `ExistsStep`, `JobStep`, `DoneStep`, `FailedJobStep`, `PrLine`, `existsAction`, `duplicateOf`, `jobSteps`, `httpsUrl` and `prNumberOf` are deleted with their tests (the shared `pr-url`, `duplicate` and `job-steps` tests cover them); `doneAction` now uses `prAddress`. Added: the PR author reads "You", the name for an Admin or "Lanekeeper user 22222222" in the page and the card (the ledger had "Another user" and "another Lanekeeper user" side by side).
- **A (attribution and severity).** 1: history rows use the shared chip and badge; **`RevertDialog` also used S3's `describeAttribution` (not in the ledger)**: it now uses the shared one through `SafeText`; S3's `ConfidenceChip`, `SeverityBadge` and `describeAttribution` are deleted (`sourceLabel` stays). 2: `FileChangedBy` at the end of the facts, `AttributionChip` in the denied-file card. 3 and 4: the three severity badges (S2, S6a, S3) are one: S2's and S3's files are deleted, S6a's too, and every importer (the finding drawer and results, the draft review, the admin severity rules) imports the shared one; `findings/severity.ts` re-exports `SEVERITIES`, `SEVERITY_LABEL`, `severityRank` (their test still runs); `FilterBar` uses `MIN_SEVERITY_CHOICES`; the Findings tab uses `nameOf` from `kinds.ts` and its address filter reads the same words (`findingTitle` and its table are deleted). 5: the Drift tab sorts by severity, shows the badge, the chip and the PR badge on a 96 px row, and has `SeverityFilter` (scope loaded, "Applies to the N files loaded so far. More are not loaded." tested with a second page that never arrives). 6: the Activity and swimlane Audit rows use `EventWho` or the chip (76 px); `actorLabel` is deleted. 7: the audit table's User and Attribution columns and the event drawer use `EventWho`, `AttributionSourceCell` and `AttributionEvidence`; S7's `whoOf`, `sourceLabel` and `confidenceView` are replaced by `eventWho` (its tests moved with the code: `event.test.ts` and the attribution model tests cover the same cases), and `nameLookup` gives the shared words the user list's name or nothing. **The ledger's expected short id was wrong** (`Lanekeeper user 11111111` for the id `11111111-...:22222222-...`; the shared `shortUserId` takes the object part, so it is `22222222`).
- **C (masking).** 1 to 4 as in the section above: `ContentView` takes `lane` (the NFS tab passes it; the Git and Baseline tabs do not, so a token and no Reveal), `HashChip copy={false}` on the withheld card. `isContentWithheld` is the one test of "withheld" in `FileScreen` and `ContentView`, and the banner uses `withheldReason`: **a file withheld for a reason other than the denied class no longer says "sensitive file type"** (it says the hub could not check it for secrets). `masking.dom.test.tsx` "the file page" had `getByText(/clientSecret/)`, which now also finds the token's screen-reader text with the setting's name; the query is the line's own text, and it also asserts the token (adapted, nothing weakened).
- **D (admin).** The mock slice is registered; `SeverityBadge` is the shared one. `KIND_INFO` stays imported from `features/findings/kinds` (that is the shared module). `e2e/visual/admin/frames.ts`: the `T5()` reason said the sections say "Not available yet"; they are built, so the reason now says so and names the DEVIATIONS row, and the 18 frames (1ao to 1bh, 1bl, 1bm) stay in `excluded` until the T5 visual specs add them as app-owned baselines (the coverage test requires every board frame to be tested or excluded with a reason). **Stage 2 must replace these 18 exclusions with frames.**

### Found by the first runs of the new code in the real app (test first, none in the ledger)
- axe `link-in-text-block` (serious) on "Find my pull request" in the job view of the PR dialog, found by the existing `a11y.spec.ts` ("file PR / the job runs and the PR is made", light and dark, 1440 and 1024): the link sat in a line of text with no underline. Both inline links of the PR views are underlined now (`underline underline-offset-2`, the app's convention), with a unit test.
- The route shim of the PR tests, the `Another Lanekeeper user` wording in four places, and the `masking.dom` ambiguity above.

### Dropped or deferred
- **C item 5 (the marker on `DiffView`, `TextResults` and setting values) is NOT applied.** Those surfaces carry no `masks` list, so a text that looks like a marker there is recognised only by its form; drawing it as a "Hidden value" token would also turn a line that merely contains that text into a token, and it would change what seven T4 tests pin (`SearchText`, `SearchSettings`, `StatusPage`, `AuditPage`, `DocsPanel`, `EffectPreview`, `ChangeSummary` assert the marker as the hub sent it). The marker holds no part of the value, so this is a reading matter and not a disclosure (threat model, residual risks).
- **No "Pull requests" link in the swimlane Detail header, no sidebar item** (the ledger's human question). The page is reached from the hub's links, "Find my pull request" in a job and "All pull requests for X" in a duplicate-PR card.
- **No copy button for a revealed value or the hash of a withheld file** (the standing human default).
- Dropped as made untrue: none of the existing DEVIATIONS rows was deleted; nine were amended (Drift tab, Findings tab, Activity and Audit tabs, File: view, File header and History, Raise PR, Raise PR job steps, Findings: severity, Admin area) and 18 added (all `pending`).

### For the contract owners (not done here)
- `job_dry_run` on `RetentionSettings` and on its input: the retention job deletes by default (D118) and the page cannot show or switch the job's own `retention_dry_run`. Recorded as a residual risk in `docs/threat-model.md`.

### What the first chromium run of the wired app found (22 failures of 1,833; none of the 22 was a flaky test; all fixed at the root)
Run on a private copy of `dist` (`LK_E2E_DIST` and `LK_DIST`), Chromium 141.0.7390.37, `--workers=2`, `LK_REACT18_DIR` set (the 94 tier-1 and tier-2 frames against the rendered handoff ran). Before the fixes: 1,611 passed, 22 failed, 177 NOT RUN (the app-owned baselines, CI only).
1. **The fail-closed view showed a wrong mock, and the new code asked for things the scenarios did not answer.** The core file's `masks` (S3's mock) were counted in characters; the first line has a middle dot (two bytes), so every offset after it was a byte short. The old view dropped the bad mask silently and drew the marker as text, which is how the frames passed; the fail-closed view hid the line ("Hidden line"; the 1d frame showed it). Fixed in `mocks/design/file/data.ts` (`lineStart` counts UTF-8 bytes), test first: `mocks/design/file/consistency.test.ts` (red: "hidden"), and `mocks/design/masks.test.ts`, a scan of every mask in every answer of every design scenario (`planMasks` must say "marks" for every line), which also found that the Git and Baseline copies of the core file carried the NFS text's masks (outside the line: the whole text refused on those tabs); fixed in `file/build.ts`.
2. **Unmocked requests (7 journeys, `csp/requests.spec`).** The wired screens ask `GET /prs` (the badges) and, for an Admin, `GET /admin/users` (the names). The Shell scenario now answers both (`SHELL_PEOPLE`: Priya and Marco; a search by `q` is answered by a match on the name or the email, as the hub does, so the palette's Users group does not change), the file view's own builder answers them (and lists the open pull request in the `exists` scenario), and the four slices that have people of their own (admin, admin-t5, approvals, attribution) take the Shell's answer out of their base (the harness answers with the first response that matches).
3. **a11y: `link-in-text-block`** on "Find my pull request" (above; 2 failures).
4. **Layout: swimlane Findings tab, labels +40%.** The title of a finding is now the Findings screen's name, which is longer, and wrapped onto a second line in a fixed 96 px row; it is one line with an ellipsis, and the pull request badge beside it does not shrink.
5. **Tier-1 File View frames (7 of 12, 0.60% to 0.76% against the limit of 0.5%).** The attribution chip is 24 px high (its target size) and the facts beside it 22, so the header grew 2 px and moved everything under it; `className="-my-px"` keeps the row at 22 px (`FileScreen`). The chip is also wider than the facts mask, so that mask now covers the whole row (`e2e/visual/file/view.spec.ts`, reason: DEVIATIONS "File header and History"). After both: 0.051% to 0.091% for the seven. The history panel row also put the Revert button on its own line once the chip was wider; it is on the source line now.
6. **Specs that asserted the old behaviour, updated:** `visual/file/view.spec.ts` "1e and 1f, Reveal: the masked value has no Reveal button" (the feature arrived; it asserts the token, a Reveal button for an Editor, and that opening the dialog and cancelling asks the hub for nothing; frames 1d to 1f themselves are compared by the T5 visual specs); `visual/file/dialogs.spec.ts` frame 1i (the three lines "Drift list now shows / PR #419 open / on this file." are said by the app now, so they leave the list of what the app does not say); `visual/admin/frames.ts` (above).

### Human decisions that are open (the defaults taken are in the code and in DEVIATIONS; none blocks)
1. Retention (D): whether the minimums (30 days, 5 versions) follow the design or the contract, and whether to add `job_dry_run` to the contract (safety: the job deletes by default and the page cannot say whether it is looking only).
2. Pull requests (B): the default filter is Open (not All); no link from the swimlane Detail header; the merged badge is not a link (the design draws it unlinked).
3. Attribution and severity (A): the Detail and File pages' severity colours follow the Findings page (the brief's choice); finding names follow `kinds.ts`; "Unknown author" wording.
4. Masking (C): no copy for a revealed value (the plan, `secret-imports.test.ts` and the standing default); no copy on the hash of a withheld file; item 5 (markers on diffs, search hits and setting values) is not applied.
5. Stage 2 must replace the 18 `excluded` Admin frames (1ao to 1bh, 1bl, 1bm) with app-owned baselines, add the PR page and the T5 surfaces to `hostile.spec`, `csp.spec`, the a11y and layout lists, and make frames 1d to 1f of the File View with `design-masking-reveal`.
6. One flake: `e2e/keyboard/keymap.spec.ts:59` ("/ goes to Search (text); typed into a field it is a character") failed once in the last full run (the `/` was pressed and the Swimlanes filter never took focus, 11.5 s); it passed in the first full run, alone (2.9 s) and 10 of 10 times with `--repeat-each=10 --workers=2`. Nothing T5 changed is on that path.

### Checkpoint, measured (Chromium 141.0.7390.37, 4 cores, a private copy of `dist` for the env-aware specs)

| Gate | Result | Before (T4) |
|---|---|---|
| `pnpm lint`, `pnpm typecheck` | clean, clean | clean |
| whole `vitest run` | 335 files, **7,417 tests** passed | 291 files, 5,914 |
| `pnpm gen:api` freshness | no diff | |
| `pnpm build` (bundle gate, dist scan) | initial JavaScript **194.8 KB** gzipped of 250 (619 KB raw), dist-check clean; `PullRequestsPage` is a 2.7 KB gzipped chunk of its own | 194.5 KB |
| `pnpm build:perf` | initial JavaScript 195.5 KB, dist-check clean (150 files) | |
| `pnpm audit` | no known vulnerabilities | |
| `cargo fmt --all -- --check` | clean | |
| chromium project, `--workers=2`, `LK_REACT18_DIR` set | **1,632 passed, 1 failed (the flake above), 177 NOT RUN** (the app-owned baselines, CI only) | 1,624 passed, 0 failed, 177 NOT RUN |
| `cargo xtask bench-check`, `just web-perf` | not run: no Rust and no perf-spec change; budgets untouched | |

`design/DEVIATIONS.md`: 158 rows, all `pending`, `scripts/deviations.test.mjs` green. `docs/threat-model.md`: Web app (08) gains "Masked values, reveal and withheld files (T5)", "Pull requests, attribution, severity and the admin sections of T5" and six residual risks.

## T5 stage 3 (the closing stage: loose ends, the gates on the official build, the one `08/T5` commit), recorded 2026-10-23

**Status.** The loose ends are done, the gates were run on the official `dist` and `dist-perf` on a quiet machine (4 cores, nothing else running), the two product defects the full run found are fixed at the root, and the commit is made. Nothing in `perf/budgets.toml`, the contract, `package.json` or the lockfile was touched. **One gate is red and needs a human: `pnpm audit` (first item under "Open human decisions").**

### Loose ends (the ledger's FINAL-STAGE TODO, each checked in the code first)
1. **`design/DEVIATIONS.md`: 165 rows, all `pending`.** Seven rows added for the layout and accessibility work (File: masked line with a control; File: revealed line, long values; Attribution chip: long names; Swimlane detail: Drift tab, row links; Swimlane detail: Findings tab, pull request badge; Admin: Retention, statistics; Admin: Sentinels and Severity rules, column widths). Amended: File: revealed value (4 px padding and fill, Hide as the secondary button), File: content withheld (the hash has no copy button in the card, the head and the History panel), Attribution chip (the design's spacing differs between its Detail and File pages), Severity badge (every screen) (Critical at 600 against 500), Pull request badges (the notes say "pull request" and "the swimlane"), Findings: severity (points at the renamed row). **A new test, `scripts/deviations.test.mjs`, "the rows that the visual specs name"**, reads every `design/DEVIATIONS.md: "Row"` reference in `e2e/visual/**` (and the `APP(...)` and `T5ROW(...)` calls) and fails on a row that does not exist, on a string of the T5 surfaces that names the file and no row, and on "candidate row" in them. Red first, with the specs as they were: 14 unresolved names ("Pull requests page", "PR job", "Duplicate PR", "PR badges", "Masks the hub gets wrong", "File: denied", "Audit: Attribution", "Severity badge" and others), 5 strings naming the file and no row, 2 "candidate row". The reasons in `attribution/{frames.ts,elements.spec.ts}`, `prs/{frames.ts,badges.spec.ts}` and `masking/{frames.ts,masking.spec.ts}` now name the real rows; the spec logic is unchanged.
2. **`ContentWithheld.tsx` was dead; deleted with its test.** The file view ships `WithheldCard` (`features/file/view/ContentView.tsx`), which matches the design's frame 1n (tier 1 compares it), so the tests moved onto the shipped card in `ContentView.test.tsx`: nothing to press, follow, type into or copy (without an author: no button at all; with one: exactly the evidence control), hidden characters in the name made visible, axe with and without an author. Mutation-checked in a scratch copy: copy on, a raw name and a download link each fail the new and the old tests. `isContentWithheld` and `withheldReason` keep their tests in `components/masking/withheld.test.ts`; `features/masking/withheld.dom.test.tsx` (the page: no edit, upload, delete, revert, PR, download, copy of the hash) is unchanged. Found while moving the tests: the dead component's tests asserted things the shipped card is not (it has a focusable hash chip with its tooltip). **`MarkedText.tsx` was also unused** (the ledger's deferred item 5: markers on diffs, search hits and setting values) and is deleted with its three tests; `splitMarkers` stays (the mask plan uses it). Item 5 stays deferred; if it is built, `MarkedText` is a ten-line component over `splitMarkers` and `HiddenValue`.
3. **`dialogs.confirms-only-when-the-primary` (`e2e/keyboard/keymap.spec.ts`) now counts confirmations.** The primitives test page counts the times each dialog asked for its write, and the spec reads the count after two frames, so "Enter did not confirm" no longer depends on a dialog being on the screen. It also covers the half of the row that had no test: Enter in the field and on the action of a consequence dialog whose action is disabled (a new "Rename file" dialog on the test page), and the numeric keypad's Enter in the destructive confirm. **Proven red by mutation** (scratch copies, `dist-perf` rebuilt each time): V1 Enter in the typed-phrase field confirms (new spec fails at "Enter in the field with the name typed in full"), V2 Enter on the destructive button confirms (fails), V3 a consequence dialog confirms with a disabled action (fails; the old spec passes it). **I could not reproduce KJ's miss**: against a rebuilt `dist-perf` the old spec also fails V1. The likely cause is that this spec serves `dist-perf` and a mutation built into `dist` only is invisible to it; I did not confirm that.
4. **Found, not fixed (as asked):** the Pull requests page subtitle can show the server's open count while a newly raised pull request is held back and not yet a row ("4 open pull requests", 3 rows). The count is the server's; the held-back row arrives when the person asks for it.

### Defects the full run found, fixed at the root with a failing test first
1. **Menus, popovers and tooltips under the sidebar (regression from stage 2, not in any earlier run).** Stage 2 made `<main>` `isolate` so that a table's sticky header (z-10) stays under a dialog's scrim. But `ui/overlay-root.ts` portals every menu, popover and tooltip into `<main>`, at z-40; a stacking context on `<main>` trapped them under the sidebar (z-5) and the header (z-6), and the account menu's items ("Sign out", "Keyboard shortcuts") could not be clicked: 6 of the 7 failures of the first full run (`csp/shell`, `csp/requests`, `live/live`). `isolate` is now on the screen's box inside `<main>` (the `div` that holds the children), not on `<main>`; the scrim order the design needs is the same. Test first: `AppShell.test.tsx` ("stacks what a screen draws inside the screen's own box, and leaves <main> ... out of that", red against `<main isolate>`), `e2e/visual/admin-t5/steps.ts` `tableHeaderStaysUnderDialog` asserts the box is `isolate` and `<main>` is not, and the Playwright specs that failed are the real proof (green).
2. **`f` right after `Home` could be dropped (the split-pane handle; T4 code).** The request for the files filter waited for the page to learn `collapsed` had changed; an `f` before the page had redrawn for `Home` saw a panel it knew was collapsed, expanded it, and the state went false, true, false inside one render, so the effect never ran and the filter never took focus. It failed once in the first full run (`keyboard/lists.spec.ts:398`, 10 s "inactive") and not in 16 + 30 repeats, which is why it looked like a flake. Both keys sent in one JS task make it deterministic: `lists.spec.ts` "split-pane-handle: f right after Home, before the page has redrawn, still puts the keyboard in the filter" failed 4 of 4, a frame between the keys passed 6 of 6. `DetailFrame.tsx` counts each request (`filterAsks`) and the effect runs for every one. After: 24 of 24 (both split tests, 12 repeats), and the second full run is green.

### Gates on the official build (Chromium 141.0.7390.37, 4 cores, 19 GB disk, nothing else running; `just` is not installed, the `verify-08` steps were run as commands)
| Gate | Result | Budget |
|---|---|---|
| `pnpm install --frozen-lockfile` | up to date | |
| `pnpm lint`, `pnpm typecheck` | clean, clean | |
| whole `vitest run` | 335 files, **7,450 tests** passed (after the last code change) | all |
| `pnpm gen:api` | no diff under `src/api` | |
| `pnpm build` (bundle gate, dist scan) | initial JavaScript **194.9 KB** gzipped (619 KB raw), 135 files scanned | 250 KB |
| `pnpm build:perf` | 195.5 KB, 150 files scanned | |
| Playwright `chromium`, `--workers=2`, `LK_REACT18_DIR` set (React 18.3.1 UMD, integrity checked against the pins of the workflow) | 2,589 tests: **2,246 passed, 0 failed, 344 NOT RUN** (the app-owned baselines, made in CI only); the first full run was 2,238 passed, 7 failed, 344 NOT RUN (defects 1 and 2 above) | all |
| route change, cached, p95, five runs, alone | **58.7, 56.0, 58.7, 63.1, 56.7 ms** (click 56.0 to 64.0, key 42.0 to 57.0, cold 34.8 to 42.1; Swimlanes p95 61.8 to 74.8, the others 22.9 to 39.7; 3 to 6 long tasks) | 100 ms |
| diff of 6,000 lines, p95, alone | **98.0 ms** (p50 81.0) | 500 ms |
| grid capacity (minimum of five), alone | **277.8 fps** (4.6 times the floor); the grid dropped 2 frames, the control list 1, allowed 9 | 60 fps |
| Lighthouse (median of three, desktop) | Compare **95**, file diff **94** (93, 94, 94), Swimlanes **96**; lowest page 94 (LCP 1.19 to 1.39 s, TBT 0 to 31.5 ms, CLS 0) | 90 |
| `cargo xtask bench-check`, the five P10 ids | 5 pass, 0 fail, 0 unmet | |
| `cargo fmt --all -- --check`; `cargo clippy -p hub --example web_fixture -- -D warnings`; `cargo test -p xtask --test budgets_registry` | clean; clean; 8 passed | |
| `pnpm audit` | **RED: 10 advisories** (see below) | none known |
| the app-owned baselines of the attribution, prs, masking, admin-t5 and admin dirs, generated into the (ignored) folder and compared, then deleted | 212 PNGs written; the compare run passed 223 of 223 (the 212 and 11 tests that need no baseline) | |

NOT RUN: the 344 app-owned baselines, made in CI (here 212 of them, the ones of the attribution, prs, masking, admin-t5 and admin dirs, were generated and compared in a throwaway folder; the other 132, of the T4 dirs, were not), `just verify`, `cargo deny`, `cargo audit` and `osv-scanner` (not installed), as asked.

### Open human decisions
1. **`pnpm audit` is red, and it is not caused by T5.** Ten advisories were published on 2026-10-20 to 22, after the last green audit (2026-10-18), against versions pinned here and unchanged: `vite` 6.4.3 (five, all in the dev server: launch-editor, `server.fs.allow`, custom middleware; patched in 6.4.4, published 2026-10-06, 17 days old: **eligible now** under the 14-day rule, a lockfile-only change inside `^6.4.3`), `vitest`/`@vitest/mocker` 4.1.11 (two, test-time mock registration; patched in 4.1.12, published 2026-10-15, 8 days old: eligible 2026-10-29), and `dompurify` 3.4.16 through `monaco-editor` (one critical, one high, one low; patched in 3.4.17, published 2026-10-12, 11 days old: eligible 2026-10-26; Monaco also ships its own vendored copy that the override cannot change, and `src/editor` does not reach DOMPurify's templates or in-place modes: see the existing note in `pnpm-workspace.yaml` and the threat model). `pnpm`'s `minimumReleaseAge` (20160 minutes) and S19 refuse two of the three today, so a clean audit is not possible under the rule before 2026-10-29. I changed nothing. Decide: wait and bump the three together in a dependency commit of their own, or record an exception for the dev-time ones now.
2. **Retention, the job's own dry-run switch.** `RetentionSettings` does not carry `job_dry_run` (D118: the job deletes by default), so the page cannot show or set it and says "read every policy here as one that deletes". A contract follow-up (`job_dry_run` on the settings and the input) is recorded in `docs/threat-model.md`.
3. **Retention minimums.** The page keeps the design's 30 days and 5 versions, which are stricter than the contract; the maximum is 10,000 versions, not the design's 1,000. Follow the design or the contract?
4. **The attribution evidence popover has no Close button.** Tab and Shift+Tab stay in it and only Escape leaves (asserted in `keyboard/t5-attribution.spec.ts`). A Close button is a one-line addition if the design owner wants one.
5. **Copy of a revealed value stays OFF** (the plan, the DEVIATIONS rows "File: revealed value" and "Copy: a value that is masked", `secret-imports.test.ts`, and now no copy of the hash of a withheld file either). Turning it on needs the display file in `SECRET_DISPLAYS`, a change to that test and a threat-model row.
6. **The route-change margin.** Five runs alone on the quiet machine gave p95 56.0 to 63.1 ms (budget 100), against T4's 92.8 ms and the five repeats of 71.4 to 130.1 ms at T4. Swimlanes with forty rows is still the slow screen (p95 61.8 to 74.8 ms, about 1.5 ms a visible row at T4). I do not know why today's figures are lower; the Swimlanes list (`features/swimlanes/SwimlanesPage.tsx`) was not changed by T5, so I would not read it as an improvement of the code. The T4 options (a lighter row, drawing the rows below the first screen a frame later, or a different budget) stay open, and the budget is unchanged.

### Residual risks
- The T4 chromium specs ran in one pass on a quiet machine; two defects that earlier private-build subsets had not seen came out of the first full run (above). Any further stage-2-style change to the shell's layers needs the full `csp/shell` and `live` specs, not a subset.
- Chromium's own background connections to `www.google.com:443` were refused by the sandbox's egress proxy during the stress runs; the app made none (`csp/requests.spec.ts` passes).
