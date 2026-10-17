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
// named, documented override in EXCEPTIONS below, never an inline comment. lint/rules.test.ts lints a bad snippet for
// every id and proves each one fires, and that each exception is as narrow as it says.

import jsxA11y from "eslint-plugin-jsx-a11y";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";

/** @typedef {{ id: string; selectors: string[]; message: string }} SyntaxRule */

/** @type {SyntaxRule[]} */
export const SYNTAX_RULES = [
  {
    id: "inner-html",
    selectors: [
      "AssignmentExpression[left.type='MemberExpression'][left.property.name=/^(innerHTML|outerHTML)$/]",
      "AssignmentExpression[left.type='MemberExpression'][left.computed=true][left.property.value=/^(innerHTML|outerHTML)$/]",
    ],
    message: "Assigning innerHTML or outerHTML is an HTML sink that Trusted Types refuses (S12). Render with React; docs HTML goes through SanitisedDocHtml only.",
  },
  {
    id: "html-parsers",
    selectors: [
      "CallExpression[callee.property.name=/^(insertAdjacentHTML|createContextualFragment)$/]",
      "NewExpression[callee.name='DOMParser']",
      "CallExpression[callee.object.name='document'][callee.property.name=/^write(ln)?$/]",
    ],
    message: "insertAdjacentHTML, createContextualFragment, DOMParser and document.write parse strings into live DOM (S12). Render with React.",
  },
  {
    id: "srcdoc",
    selectors: ["AssignmentExpression[left.property.name='srcdoc']", "JSXAttribute[name.name='srcDoc']"],
    message: "srcdoc runs a string as a page (S12). The app has no iframes.",
  },
  {
    id: "dangerous-elements",
    selectors: ["CallExpression[callee.property.name='createElement'][arguments.0.value=/^(script|iframe|object|embed)$/i]"],
    message: "Creating a script, iframe, object or embed element is code or content from outside (S12). Bundle code with Vite.",
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
    selectors: ["CallExpression[callee.name='eval']", "NewExpression[callee.name='Function']", "CallExpression[callee.name='Function']"],
    message: "eval and new Function are refused by the CSP (no 'unsafe-eval').",
  },
  {
    id: "navigation",
    selectors: [
      "CallExpression[callee.object.name='window'][callee.property.name='open'][arguments.0.type!='Literal']",
      "AssignmentExpression[left.object.name='location'][right.type!='Literal']",
      "AssignmentExpression[left.object.name='window'][left.property.name='location'][right.type!='Literal']",
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
    selectors: ["CallExpression[callee.property.name='createPolicy']"],
    message: "Only src/lib/csp/trusted-types.ts creates Trusted Types policies, each once (the hub allows two names).",
  },
  {
    id: "web-storage",
    selectors: ["Identifier[name=/^(localStorage|sessionStorage|indexedDB|openDatabase)$/]"],
    message: "Web storage is for src/lib/prefs only (S2, S10): layout choices, never API data. Use the pref helpers.",
  },
  {
    id: "cookie-write",
    selectors: ["AssignmentExpression[left.type='MemberExpression'][left.property.name='cookie']"],
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
    selectors: [
      "CallExpression[callee.name='fetch']",
      "CallExpression[callee.object.name=/^(window|globalThis|self)$/][callee.property.name='fetch']",
      "NewExpression[callee.name=/^(XMLHttpRequest|WebSocket|EventSource|SharedWorker)$/]",
      "CallExpression[callee.property.name='sendBeacon']",
    ],
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
    for (const selector of rule.selectors) entries.push({ selector, message: `[lk/${rule.id}] ${rule.message}` });
  }
  return ["error", ...entries];
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
      // `role` and `aria-*` props of our own components (RoleBadge role="editor") are not ARIA roles.
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
    rules: { "no-restricted-syntax": syntaxRule() },
  },
  ...EXCEPTIONS.map(({ name, files, allow, rules }) => ({
    name: `lanekeeper/exception/${name}`,
    files,
    rules: { "no-restricted-syntax": syntaxRule(allow), ...rules },
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
    rules: { "no-restricted-syntax": "off", "no-script-url": "off", "no-control-regex": "off" },
  },
  {
    // Node tooling: scripts, lint, e2e and config files run in Node or in Playwright, not in the page.
    name: "lanekeeper/tooling",
    files: ["scripts/**", "lint/**", "e2e/**", "*.config.{js,ts,mjs}"],
    rules: { "no-console": "off" },
  },
];
