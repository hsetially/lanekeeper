// Lanekeeper's own lint rules (S21 equivalents for the web app), as flat-config blocks that eslint.config.js spreads.
//
// What they protect, in one sentence each:
//  - nothing writes HTML, script or a style string into the page (S12: Trusted Types and the nonce-only CSP would refuse
//    it at run time; the lint refuses it at build time, with the reason);
//  - nothing but src/api/client.ts talks to the network, and nothing but src/lib/prefs touches web storage (S2, S10);
//  - nothing builds a navigation, a regular expression or a policy from data it does not control;
//  - no `eslint-disable` comment can switch any of this off inside src (noInlineConfig).
//
// Every restriction has an id. A file that must break one (the one HTML sink, the one fetch, the Monaco patches) gets a
// named, documented override in EXCEPTIONS below, never an inline comment. lint/rules.test.mjs lints a bad snippet for
// every id and proves each one fires, and that each exception is as narrow as it says.
//
// A restriction is written in one or both of two forms, and both are switched off together by an exception:
//  - `selectors`: a shape of code (no-restricted-syntax). Used for members, keys and strings, where the name is the whole
//    point (`insertAdjacentHTML`, `{ innerHTML }`, `'srcdoc'`), whoever the object is;
//  - `globals`: a name that is the browser's own (no-restricted-globals with checkGlobalObject). Scope-aware, so a local or a
//    property called `fetch` is fine, and `window.fetch`, `globalThis["fetch"]` and `const f = fetch` are all the one global.
//
// What this is NOT: a proof. It reads how code is written, so a name built at run time, the global object held in another
// variable (`const w = window; w.fetch(..)`) or `(() => {}).constructor("..")` are invisible to it (lint/rules.test.mjs lists
// these limits). The guarantee that does not depend on how the code is written is the browser's: Trusted Types refuses a string at
// every HTML and script sink, the CSP refuses eval, off-site requests and inline script and style, and the server sanitises the
// docs HTML. docs/threat-model.md ("Web app (08)", the S12-T row) says which layer holds for what.

import jsxA11y from "eslint-plugin-jsx-a11y";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";

/** @typedef {{ id: string; selectors?: string[]; globals?: string[]; message: string }} SyntaxRule */

// The words a name can be written as when it is a member, a key or a string: `x.name`, `{ name }`, `x["name"]`, `` x[`name`] ``.
const named = (/** @type {string} */ pattern) => [`Identifier[name=/^(${pattern})$/]`, `Literal[value=/^(${pattern})$/]`, `TemplateElement[value.raw=/^(${pattern})$/]`];

/** @type {SyntaxRule[]} */
export const SYNTAX_RULES = [
  {
    id: "inner-html",
    selectors: [
      "AssignmentExpression[left.type='MemberExpression'][left.property.name=/^(innerHTML|outerHTML)$/]",
      // Reading `el.innerHTML` is fine. Writing it by another door is not: a property bag (Object.assign(el, { innerHTML })), a
      // reflective set, a key held in a variable or a computed access all mention the name as a key or as a string.
      "ObjectExpression > Property[key.name=/^(innerHTML|outerHTML)$/]",
      "Literal[value=/^(innerHTML|outerHTML)$/]",
      "TemplateElement[value.raw=/^(innerHTML|outerHTML)$/]",
    ],
    message: "Assigning innerHTML or outerHTML, by any route, is an HTML sink that Trusted Types refuses (S12). Render with React; docs HTML goes through SanitisedDocHtml only.",
  },
  {
    id: "html-parsers",
    selectors: [
      // Every mention, not only a call: an alias, a destructuring and a computed name reach the parser as well.
      ...named("insertAdjacentHTML|createContextualFragment|setHTMLUnsafe|parseHTMLUnsafe|setHTML"),
      "MemberExpression[object.name='document'][property.name=/^write(ln)?$/]",
      "MemberExpression[object.name='document'][computed=true][property.value=/^write(ln)?$/]",
    ],
    globals: ["DOMParser"],
    message: "insertAdjacentHTML, createContextualFragment, setHTML, setHTMLUnsafe, parseHTMLUnsafe, DOMParser and document.write parse strings into live DOM (S12). Render with React.",
  },
  {
    id: "srcdoc",
    selectors: [
      "AssignmentExpression[left.property.name='srcdoc']",
      "JSXAttribute[name.name='srcDoc']",
      "CallExpression[callee.property.name='setAttribute'][arguments.0.value=/^srcdoc$/i]",
      "ObjectExpression > Property[key.name='srcdoc']",
    ],
    message: "srcdoc runs a string as a page (S12). The app has no iframes.",
  },
  {
    id: "dangerous-elements",
    selectors: [
      "CallExpression[callee.property.name='createElement'][arguments.0.value=/^(script|iframe|object|embed)$/i]",
      "CallExpression[callee.property.name='createElementNS'][arguments.1.value=/^(script|iframe|object|embed)$/i]",
      // A tag name that is not written out can be any of them. (React.createElement takes a component, not a tag name.)
      "CallExpression[callee.property.name='createElement'][callee.object.name!='React'][arguments.0.type!='Literal']",
      "CallExpression[callee.property.name='createElementNS'][arguments.1.type!='Literal']",
    ],
    message: "Creating a script, iframe, object or embed element is code or content from outside (S12), and so is an element whose tag name is not written out. Bundle code with Vite.",
  },
  {
    id: "event-handler-property",
    selectors: [
      "AssignmentExpression[left.type='MemberExpression'][left.property.name=/^on[a-z]+$/]",
      "CallExpression[callee.property.name='setAttribute'][arguments.0.value=/^on/i]",
    ],
    message: "Assigning an on* handler property or attribute is an inline handler (S12). Use addEventListener or a React prop.",
  },
  {
    id: "set-attribute-style",
    selectors: ["CallExpression[callee.property.name='setAttribute'][arguments.0.value=/^style$/i]"],
    message: "setAttribute('style') is blocked by the CSP (style-src has no 'unsafe-inline'). Use the style prop, el.style.x, or a token class.",
  },
  {
    id: "javascript-url",
    selectors: ["Literal[value=/^\\s*javascript:/i]", "TemplateElement[value.raw=/^\\s*javascript:/i]"],
    message: "A javascript: URL is script (S12).",
  },
  {
    id: "eval",
    // The globals cover eval(x), window.eval(x), (0, eval)(x), const run = eval, new Function(..), globalThis["Function"](..).
    globals: ["eval", "Function"],
    message: "eval and new Function are refused by the CSP (no 'unsafe-eval').",
  },
  {
    id: "navigation",
    selectors: [
      "CallExpression[callee.object.name=/^(window|globalThis|self)$/][callee.property.name='open'][arguments.0.type!='Literal']",
      "AssignmentExpression[left.object.name='location'][right.type!='Literal']",
      "AssignmentExpression[left.object.name=/^(window|globalThis|self|document)$/][left.property.name='location'][right.type!='Literal']",
      "AssignmentExpression[left.object.property.name='location'][right.type!='Literal']",
      "AssignmentExpression[left.name='location'][right.type!='Literal']",
      "CallExpression[callee.object.name='location'][callee.property.name=/^(assign|replace)$/][arguments.0.type!='Literal']",
      "CallExpression[callee.object.property.name='location'][callee.property.name=/^(assign|replace)$/][arguments.0.type!='Literal']",
    ],
    message: "Navigate with the router or a link. window.open and a location assignment with a value that is not a literal are open-redirect and javascript: URL sinks.",
  },
  {
    id: "dynamic-regexp",
    selectors: ["NewExpression[callee.name='RegExp'][arguments.0.type!='Literal']", "CallExpression[callee.name='RegExp'][arguments.0.type!='Literal']"],
    message: "A regular expression built from data is a ReDoS and injection risk (S11). A user's pattern goes to the server, which matches in linear time.",
  },
  {
    id: "create-policy",
    // An access or a destructuring, not a declaration: src/lib/csp/trusted-types-dom.d.ts declares the member and may.
    selectors: [
      "MemberExpression[property.name='createPolicy']",
      "MemberExpression[computed=true][property.value='createPolicy']",
      "ObjectPattern > Property[key.name='createPolicy']",
    ],
    message: "Only src/lib/csp/trusted-types.ts creates Trusted Types policies, each once (the hub allows two names).",
  },
  {
    id: "web-storage",
    // By name wherever it is written (a property, a destructuring, `window["localStorage"]`); the Cache Storage is the global `caches`.
    selectors: named("localStorage|sessionStorage|indexedDB|openDatabase"),
    globals: ["caches"],
    message: "Web storage (localStorage, sessionStorage, IndexedDB, the Cache Storage) is for src/lib/prefs only (S2, S10): layout choices, never API data. Use the pref helpers.",
  },
  {
    id: "cookie-write",
    selectors: ["AssignmentExpression[left.type='MemberExpression'][left.property.name='cookie']"],
    globals: ["cookieStore"],
    message: "Cookies are written by the hub, except the theme choice in src/lib/prefs.",
  },
  {
    id: "form-submit",
    selectors: ["JSXOpeningElement[name.name='form'] > JSXAttribute[name.name=/^(method|action)$/]"],
    message: "A <form method> navigation sends `Origin: null` under no-referrer and the CSRF layer refuses it. Submit with fetch through the generated client.",
  },
  {
    id: "optimistic-update",
    selectors: ["Property[key.name='onMutate']", "CallExpression[callee.property.name=/^(setQueryData|setQueriesData)$/]"],
    message:
      "No optimistic updates and no setQueryData: a write shows the server's answer (it may be hash-guarded, sent for approval, or carry a secret, and a secret must never be written into the cache). The one exception is src/lib/query/optimistic.ts, a closed list of operations the contract shows to be safe; use optimisticWriteMutation for those.",
  },
  {
    id: "query-key-literal",
    selectors: [
      "Property[key.name='queryKey'][value.type='ArrayExpression']",
      "CallExpression[callee.property.name=/^(getQueryData|setQueryData|getQueriesData|setQueriesData|getQueryState|removeQueries|resetQueries|cancelQueries|invalidateQueries|refetchQueries|fetchQuery|prefetchQuery|ensureQueryData|fetchInfiniteQuery|prefetchInfiniteQuery|ensureInfiniteQueryData|isFetching)$/][arguments.0.type='ArrayExpression']",
      "ImportSpecifier[imported.name=/^get[A-Za-z0-9]+Query(Key|Options)$/]",
    ],
    message:
      "Build a query key with the factories of src/lib/query/keys.ts (swimlaneKeys.detail(id), fileKeys.head(id, path), ...), never a literal array and never the key the generator made (getXQueryKey). The live events and the writes invalidate by those keys; a key of any other shape is a cache they can never find.",
  },
  {
    id: "unfiltered-invalidation",
    selectors: [
      "CallExpression[callee.property.name=/^(invalidateQueries|refetchQueries|resetQueries|cancelQueries|removeQueries)$/][arguments.length=0]",
      "CallExpression[callee.property.name=/^(invalidateQueries|refetchQueries|resetQueries|cancelQueries|removeQueries)$/][arguments.length>0][arguments.0.type!='ObjectExpression']",
      "CallExpression[callee.property.name=/^(invalidateQueries|refetchQueries|resetQueries|cancelQueries|removeQueries)$/][arguments.0.type='ObjectExpression']:not(:has(Property[key.name=/^(queryKey|predicate)$/]))",
    ],
    message:
      "Invalidate with a filter: a `queryKey` built from a factory of src/lib/query/keys.ts, or a `predicate`. A call that names nothing refetches every query on screen, which is what a live event must never do (P13). The write table (src/lib/query/invalidate.ts) says what each write invalidates.",
  },
  {
    id: "network",
    // Scope-aware: any use of the global (a call, an alias, a reference passed on, `window.fetch`, `globalThis["fetch"]`).
    globals: ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "SharedWorker"],
    selectors: named("sendBeacon"),
    message: "Only src/api/client.ts (and the stream and warm-up clients named in lint/lanekeeper-rules.mjs) talk to the network. Use the generated hooks.",
  },
  {
    id: "worker",
    selectors: [
      "NewExpression[callee.name=/^(Worker|SharedWorker)$/][arguments.0.type!='Literal']",
      "CallExpression[callee.property.name='createObjectURL']",
      "CallExpression[callee.object.property.name='serviceWorker'][callee.property.name='register']",
    ],
    message: "Workers, object URLs and service workers belong to src/editor only (a worker needs a TrustedScriptURL from the editor policy, and worker-src is 'self').",
  },
  {
    id: "global-patch",
    selectors: [
      "AssignmentExpression[left.type='MemberExpression'][left.object.type='MemberExpression'][left.object.property.name='prototype']",
      "CallExpression[callee.object.name='Object'][callee.property.name=/^(defineProperty|defineProperties|setPrototypeOf)$/][arguments.0.type='MemberExpression'][arguments.0.property.name='prototype']",
    ],
    message: "Patching a built-in prototype changes the whole page. Only src/editor/global-patches.ts may (Monaco under the strict CSP); see docs/threat-model.md.",
  },
  {
    id: "messaging",
    selectors: [...named("postMessage"), "CallExpression[callee.property.name='addEventListener'][arguments.0.value='message']"],
    globals: ["BroadcastChannel", "MessageChannel"],
    message:
      "The page talks to no other window, tab or frame (no iframes, COOP same-origin, no service worker), so a message channel is an entry point that nothing authenticates. Use the router, the query cache or the generated client.",
  },
];

/**
 * A `no-restricted-syntax` value with every rule except those named. A file that is allowed to do one of these things
 * gets this with the id left out, so the exception is exactly as wide as its entry in EXCEPTIONS.
 * @param {string[]} [except]
 */
export function syntaxRule(except = []) {
  const entries = [];
  for (const rule of SYNTAX_RULES) {
    if (except.includes(rule.id)) continue;
    for (const selector of rule.selectors ?? []) entries.push({ selector, message: `[lk/${rule.id}] ${rule.message}` });
  }
  return ["error", ...entries];
}

/**
 * A `no-restricted-globals` value with the globals of every rule except those named: the same exception list as
 * `syntaxRule`, for the restrictions that are about a name the browser owns. `checkGlobalObject` makes `window.x`,
 * `globalThis["x"]` and `self.x` the same global.
 * @param {string[]} [except]
 */
export function globalsRule(except = []) {
  const globals = [];
  for (const rule of SYNTAX_RULES) {
    if (except.includes(rule.id)) continue;
    for (const name of rule.globals ?? []) globals.push({ name, message: `[lk/${rule.id}] ${rule.message}` });
  }
  return ["error", { globals, checkGlobalObject: true }];
}

/**
 * The named exceptions. Each says which restriction a file may break and why. Adding one is a security review item.
 * @type {Array<{ name: string; files: string[]; allow: string[]; why: string; rules?: Record<string, string> }>}
 */
export const EXCEPTIONS = [
  // Order matters: a later block wins for a file both match, so a broad entry comes first and each narrower one lists
  // everything its file may do.
  { name: "editor-workers", files: ["src/editor/**/*.ts"], allow: ["worker"], why: "The editor's web worker is created through a TrustedScriptURL from the editor policy." },
  { name: "api-client", files: ["src/api/client.ts"], allow: ["network"], why: "The one mutator: every API request is made here, with CSRF, an Idempotency-Key, a timeout and problem parsing." },
  { name: "sse-client", files: ["src/live/sse-client.ts"], allow: ["network"], why: "The live-event stream is a long fetch read through a parser (T3). EventSource has no header control and no status codes." },
  { name: "editor-warm-up", files: ["src/editor/warm.ts"], allow: ["network"], why: "A fetch-only warm-up of the editor chunk for editors (T2): it fetches the hashed files into the HTTP cache while the browser is idle and never imports or executes them." },
  {
    name: "optimistic-helper",
    files: ["src/lib/query/optimistic.ts"],
    allow: ["optimistic-update"],
    why:
      "The only code that may touch the cache while a write is in flight: it cancels what is loading, remembers, shows the expected answer, rolls back on failure and ALWAYS refetches on settle. It serves a closed registry of operations that the contract shows to be safe (a PUT with the whole value, no 202, 409, 412 or 423, no expected hash, not secret, not a reveal); optimistic.test.ts checks the registry against api/openapi.yaml.",
  },
  { name: "trusted-types-module", files: ["src/lib/csp/trusted-types.ts"], allow: ["create-policy"], why: "The only module that creates Trusted Types policies, each once." },
  { name: "prefs-module", files: ["src/lib/prefs/**/*.ts"], allow: ["web-storage", "cookie-write"], why: "The only module that touches web storage (layout choices) and the theme cookie." },
  {
    name: "monaco-global-patches",
    files: ["src/editor/global-patches.ts"],
    allow: ["global-patch", "worker"],
    // Patching a built-in means reading it through its prototype (unbound by design) and wrapping the `string` overload of
    // createElement, which the DOM typings mark deprecated in favour of the tag-name overloads that cannot express a wrapper.
    rules: { "@typescript-eslint/unbound-method": "off", "@typescript-eslint/no-deprecated": "off" },
    why:
      "Monaco 0.57 under the hub's CSP (no style-src-attr, Trusted Types, nonce-only style-src) needs three patches of built-ins: a nonce on the <style> elements it creates (Document.prototype.createElement), setAttribute('style') routed to CSSOM (Element.prototype.setAttribute) and a MutationObserver that applies the style data its createHTML rewrote. They load only with the lazy editor chunk, are measured by e2e/editor/csp.spec.ts on every Monaco upgrade, and are recorded in docs/threat-model.md.",
  },
];

const testFiles = ["src/**/*.test.{ts,tsx}", "src/test/**/*.{ts,tsx}", "src/mocks/**/*.{ts,tsx}"];
const productFiles = ["src/**/*.{ts,tsx}"];
const browserFiles = ["src/**/*.{ts,tsx}"];

/** Flat-config blocks. Type-aware presets are added by eslint.config.js; these need no type information. */
export const lanekeeperRules = [
  {
    files: productFiles,
    ignores: ["src/api/generated/**", "**/*.generated.ts"],
    // No `eslint-disable` inside src: an exception is a named block above, reviewed with the rules.
    linterOptions: { noInlineConfig: true, reportUnusedDisableDirectives: "error" },
  },
  {
    files: browserFiles,
    // The global object under its three names. The base `no-eval` only knows `window.eval(x)` when `window` is a declared global,
    // and the restricted globals below look through these names too; declaring them here gives every config the same scopes.
    languageOptions: { globals: { window: "readonly", self: "readonly", globalThis: "readonly" } },
    plugins: { react, "jsx-a11y": jsxA11y, "react-hooks": reactHooks },
    settings: { react: { version: "detect" } },
    rules: {
      ...jsxA11y.flatConfigs.recommended.rules,
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      // S12: no raw HTML injection. The one exception is the server-sanitised docs renderer (EXCEPTIONS-style block below).
      "react/no-danger": "error",
      "react/no-danger-with-children": "error",
      "react/jsx-no-script-url": "error",
      "react/jsx-no-target-blank": ["error", { enforceDynamicLinks: "always", warnOnSpreadAttributes: true }],
      "react/iframe-missing-sandbox": "error",
      // `role` and `aria-*` props of our own components (a `role` prop that holds a Lanekeeper role, "editor") are not ARIA roles.
      "jsx-a11y/aria-role": ["error", { ignoreNonDOM: true }],
      // Our form controls are components; a <label> around one of them is a labelled control.
      "jsx-a11y/label-has-associated-control": ["error", { controlComponents: ["Input", "Textarea", "NativeSelect", "Checkbox", "Switch", "RadioGroupItem"], depth: 3 }],
      "no-console": "error",
      "no-debugger": "error",
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
      "no-script-url": "error",
    },
  },
  {
    files: productFiles,
    ignores: [...testFiles, "src/api/generated/**", "**/*.generated.ts"],
    rules: { "no-restricted-syntax": syntaxRule(), "no-restricted-globals": globalsRule() },
  },
  ...EXCEPTIONS.map(({ name, files, allow, rules }) => ({
    name: `lanekeeper/exception/${name}`,
    files,
    rules: { "no-restricted-syntax": syntaxRule(allow), "no-restricted-globals": globalsRule(allow), ...rules },
  })),
  {
    // The one component that may set inner HTML: it renders RenderedDoc.html, which the server sanitised, through the
    // Trusted Types policy lanekeeper-docs. SanitisedDocHtml.test.tsx proves it is the only one.
    name: "lanekeeper/exception/sanitised-doc-html",
    files: ["src/components/app/SanitisedDocHtml.tsx"],
    rules: { "react/no-danger": "off" },
  },
  {
    // Truncated text shows its full value in a tooltip. A keyboard user must be able to focus the text to see it (WCAG
    // 1.4.13), and the tooltip trigger is the focus stop, so this one span carries a tabIndex.
    name: "lanekeeper/exception/tooltip-focus-stop",
    files: ["src/components/app/chips.tsx"],
    rules: { "jsx-a11y/no-noninteractive-tabindex": "off" },
  },
  {
    // Tests, mocks and test helpers stub the network and storage and read the DOM; the browser rules are for product code.
    // Test data holds hostile strings on purpose (javascript: URLs, control characters).
    name: "lanekeeper/tests",
    files: testFiles,
    rules: { "no-restricted-syntax": "off", "no-restricted-globals": "off", "no-script-url": "off", "no-control-regex": "off" },
  },
  {
    // Node tooling: scripts, lint, e2e and config files run in Node or in Playwright, not in the page.
    name: "lanekeeper/tooling",
    files: ["scripts/**", "lint/**", "e2e/**", "*.config.{js,ts,mjs}"],
    rules: { "no-console": "off" },
  },
];
