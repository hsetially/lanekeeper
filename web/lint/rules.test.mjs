import { join } from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";
import { WEB_ROOT } from "../scripts/handoff/dc-parse.mjs";
import { EXCEPTIONS, SYNTAX_RULES } from "./lanekeeper-rules.mjs";
import { syntaxOnlyConfig } from "./syntax-config.mjs";

// T1 Verify: "the lint ban works". Every restriction has a bad snippet here, and the test proves it fires with its id;
// every named exception is proved to be exactly as wide as it says.

const eslint = new ESLint({ cwd: WEB_ROOT, overrideConfigFile: true, overrideConfig: syntaxOnlyConfig });
const PRODUCT = "src/features/x/Page.tsx";

async function lint(code, file = PRODUCT) {
  const [result] = await eslint.lintText(code, { filePath: join(WEB_ROOT, file) });
  return result.messages;
}
// The restrictions come from two rules: no-restricted-syntax (a shape of code) and no-restricted-globals (a name that is the browser's,
// scope-aware, also through window, globalThis and self). Both carry `[lk/<id>]` in the message.
const LK_RULES = new Set(["no-restricted-syntax", "no-restricted-globals"]);
const ids = (messages) => messages.filter((m) => LK_RULES.has(m.ruleId)).map((m) => /\[lk\/([a-z-]+)\]/.exec(m.message)?.[1]);
const rules = (messages) => messages.map((m) => m.ruleId);

/** id -> snippets that must each produce that id. */
const BAD = {
  "inner-html": [
    "el.innerHTML = x;",
    "el.outerHTML = '<b>';",
    "el['innerHTML'] = x;",
    "document.body.innerHTML += x;",
    // The same sink by another door: a property bag, a reflective set, a key held in a variable.
    "Object.assign(el, { innerHTML });",
    "Object.assign(el, { innerHTML: x });",
    "Object.assign(el, { ['outerHTML']: x });",
    "Reflect.set(el, 'innerHTML', x);",
    "Object.defineProperty(el, 'innerHTML', { set: f });",
    "const key = 'innerHTML'; el[key] = x;",
    "el[`innerHTML`] = x;",
  ],
  "html-parsers": [
    "el.insertAdjacentHTML('beforeend', x);",
    "range.createContextualFragment(x);",
    "new DOMParser().parseFromString(x, 'text/html');",
    "document.write(x);",
    "document.writeln(x);",
    // The newer parsers (setHTMLUnsafe skips even the sanitiser), and every way of reaching a parser without a plain call.
    "el.setHTMLUnsafe(x);",
    "Document.parseHTMLUnsafe(x);",
    "el.setHTML(x);",
    "el['setHTMLUnsafe'](x);",
    "el['insertAdjacentHTML']('beforeend', x);",
    "const { insertAdjacentHTML } = el;",
    "const w = document.write;",
    "document['write'](x);",
    "new window.DOMParser();",
    "const P = DOMParser; new P();",
  ],
  srcdoc: ["frame.srcdoc = x;", "const a = <iframe srcDoc={x} sandbox='' />;", "frame.setAttribute('srcdoc', x);", "frame.setAttribute('SRCDOC', x);", "Object.assign(frame, { srcdoc: x });"],
  "dangerous-elements": [
    "document.createElement('script');",
    "document.createElement('IFRAME');",
    "document.createElement('object');",
    "document.createElement('embed');",
    // Namespaced elements are the same elements; a tag name held in a variable can be any of them.
    "document.createElementNS('http://www.w3.org/1999/xhtml', 'script');",
    "document.createElementNS(ns, 'iframe');",
    "document.createElementNS(ns, tag);",
    "document.createElement(tag);",
    "doc.createElement(`${tag}`);",
  ],
  "event-handler-property": ["el.onclick = f;", "window.onerror = f;", "el.setAttribute('onload', 'x()');"],
  "set-attribute-style": ["el.setAttribute('style', 'top:0');", "el.setAttribute('STYLE', x);"],
  "javascript-url": ["const u = 'javascript:alert(1)';", "const u = ' JavaScript:alert(1)';", "const u = `javascript:${x}`;"],
  eval: [
    "eval(x);",
    "new Function('return 1');",
    "Function('return 1')();",
    // Through the global object (the base no-eval is told what `window` is, and the restricted global sees through it), indirect, aliased.
    "window.eval(x);",
    "globalThis.eval(x);",
    "self['eval'](x);",
    "(0, eval)(x);",
    "const run = eval; run(x);",
    "new window.Function('return 1');",
    "globalThis['Function']('return 1')();",
  ],
  navigation: [
    "window.open(x);",
    "location.href = x;",
    "window.location = x;",
    "window.location.href = x;",
    "location = x;",
    "location.assign(x);",
    "window.location.replace(x);",
    "globalThis.open(x);",
    "document.location = x;",
    "globalThis.location.href = x;",
    "self.location.assign(x);",
  ],
  "dynamic-regexp": ["new RegExp(q);", "RegExp(q, 'i');", "new RegExp(`^${q}`);"],
  "create-policy": ["window.trustedTypes.createPolicy('x', {});", "trustedTypes.createPolicy('x', {});", "trustedTypes['createPolicy']('x', {});", "const { createPolicy } = trustedTypes;"],
  "web-storage": [
    "localStorage.setItem('a', 'b');",
    "window.sessionStorage.clear();",
    "indexedDB.open('x');",
    // By computed access, through the global object, by alias, and the other persistent stores.
    "window['localStorage'].clear();",
    "globalThis['sessionStorage'].clear();",
    "self.indexedDB.open('x');",
    "const store = localStorage;",
    "const { sessionStorage } = window;",
    "caches.open('x');",
    "window.caches.delete('x');",
    "globalThis['caches'].open('x');",
  ],
  "cookie-write": ["document.cookie = 'a=b';", "cookieStore.set('a', 'b');", "window.cookieStore.delete('a');"],
  "form-submit": ["const f = <form method='post'><button>x</button></form>;", "const f = <form action='/x' />;"],
  "optimistic-update": ["useMutation({ onMutate: () => {} });", "client.setQueryData(key, 1);", "client.setQueriesData({ queryKey: key }, f);"],
  "query-key-literal": [
    "useQuery({ queryKey: ['swimlanes'], queryFn: f });",
    "useQuery({ queryKey: ['swimlane', id], queryFn: f });",
    "client.getQueryData(['swimlanes']);",
    "client.setQueryData(['swimlane', id], d);",
    "client.invalidateQueries({ queryKey: ['tree', id] });",
    "client.invalidateQueries(['tree']);",
    "client.removeQueries(['compare']);",
    "client.prefetchQuery({ queryKey: ['me'], queryFn: f });",
    "client.getQueryState(['me']);",
    "client.cancelQueries(['grid']);",
    "import { getListSwimlanesQueryKey } from '@/api/generated/endpoints/swimlanes/swimlanes';",
    "import { getGetSwimlaneQueryOptions } from '@/api/generated/endpoints/swimlanes/swimlanes';",
  ],
  "unfiltered-invalidation": [
    "client.invalidateQueries();",
    "client.invalidateQueries({});",
    "client.invalidateQueries({ refetchType: 'all' });",
    "client.invalidateQueries(filters);",
    "client.refetchQueries();",
    "client.resetQueries();",
    "client.cancelQueries();",
    "client.removeQueries({ type: 'all' });",
  ],
  network: [
    "fetch('/api/v1/x');",
    "fetch(`/api/v1/${id}`);",
    "window.fetch(u);",
    "globalThis.fetch(u);",
    "new XMLHttpRequest();",
    "new WebSocket(u);",
    "new EventSource(u);",
    "navigator.sendBeacon(u, d);",
    // The same requests without the plain call: an alias, a computed name, a reference passed on, the global object under any name.
    "const f = fetch; f('/api/v1/x');",
    "globalThis['fetch']('/api/v1/x');",
    "window['fetch'](u);",
    "self.fetch(u);",
    "const f = window.fetch;",
    "run(fetch);",
    "new window.WebSocket(u);",
    "const S = EventSource; new S(u);",
    "navigator['sendBeacon'](u, d);",
    "const { sendBeacon } = navigator;",
  ],
  worker: ["new Worker(url);", "new SharedWorker(url);", "URL.createObjectURL(blob);", "navigator.serviceWorker.register(url);"],
  "global-patch": ["Element.prototype.setAttribute = f;", "Document.prototype.createElement = f;", "Object.defineProperty(HTMLElement.prototype, 'x', {});"],
  // The page talks to no other window, tab or frame (no iframes, COOP same-origin, no service worker), so a message channel is
  // an entry point that nothing authenticates.
  messaging: [
    "window.postMessage(x, '*');",
    "postMessage(x);",
    "window['postMessage'](x, origin);",
    "worker.postMessage(x);",
    "window.addEventListener('message', f);",
    "self.addEventListener('message', f);",
    "new BroadcastChannel('x');",
    "const { port1 } = new MessageChannel();",
    "window.BroadcastChannel;",
  ],
};

describe("every restriction fires, with its id", () => {
  it("covers every id of the rule list, and no other", () => {
    expect(Object.keys(BAD).sort()).toEqual(SYNTAX_RULES.map((r) => r.id).sort());
  });

  for (const [id, snippets] of Object.entries(BAD)) {
    it.each(snippets)(`${id}: %s`, async (code) => {
      const found = ids(await lint(`${code}\n`));
      expect(found, code).toContain(id);
    });
  }

  it("says why in the message, not just that it is banned", async () => {
    const [m] = (await lint("el.innerHTML = x;\n")).filter((x) => LK_RULES.has(x.ruleId));
    expect(m?.message).toMatch(/Trusted Types/);
  });
});

describe("what is allowed", () => {
  it.each([
    "const a = el.innerHTML;",
    "const u = new URL(x, location.origin);",
    "navigate({ to: '/swimlanes' });",
    "window.open('https://github.com/settings/personal-access-tokens', '_blank', 'noopener,noreferrer');",
    "location.href = '/auth/login';",
    "const r = new RegExp('^a+$');",
    "el.style.top = '4px';",
    "el.setAttribute('aria-label', 'x');",
    "el.addEventListener('click', f);",
    "const w = new Worker('/x.js');",
    // Names the restrictions are about, used as something else: a property, a key, a type, a local of the same name.
    "const { innerHTML } = el;",
    "const o = { fetch: (cursor, signal) => get(cursor, signal), once: true };",
    "const f = options.fetch ?? run;",
    "const fetchPage = (fetch) => fetch(1);",
    "function load(fetch) { return fetch('/x'); }",
    "type Fetcher = typeof fetch;",
    "const a = React.createElement(Comp, props);",
    "const b = createElement(Comp, props);",
    "const a = document.createElement('a');",
    "window.addEventListener('resize', f);",
    "window.matchMedia('(min-width: 1px)');",
    "window.setTimeout(f, 10);",
    "globalThis.crypto.randomUUID();",
    "window.scrollTo(0, 0);",
    "const e = new CustomEvent('x', { detail: 1 });",
    "el.dispatchEvent(e);",
    "interface TrustedTypePolicyFactory { createPolicy(name: string): Policy }",
    "type Msg = { postMessageCount: number };",
  ])("passes %s", async (code) => {
    // `new Worker('/x.js')` has a literal argument; the worker rule is for built URLs.
    expect(ids(await lint(`${code}\n`)), code).toEqual([]);
  });

  it("does not apply the browser restrictions to tests, mocks and test helpers", async () => {
    for (const file of ["src/api/client.test.ts", "src/mocks/handlers.ts", "src/test/setup.ts", "src/components/app/X.test.tsx"]) {
      expect(ids(await lint("localStorage.clear(); fetch(u); el.innerHTML = x;\n", file)), file).toEqual([]);
    }
  });

  it("does not apply them to Node tooling (scripts, e2e)", async () => {
    for (const file of ["scripts/x.mjs", "e2e/csp/x.ts", "playwright.config.ts"]) {
      expect(ids(await lint("fetch(u); el.innerHTML = x;\n", file)), file).toEqual([]);
    }
  });
});

// The lint reads the shape of the code, with scopes for names the browser owns. It is a build-time guard against the ways of
// writing these things that people and agents actually use, not a proof, and docs/threat-model.md ("Web app (08)", row S12-T)
// says so and says which layer holds for each of the following: Trusted Types (a string is refused at the sink), the CSP (no
// 'unsafe-eval', connect-src 'self', no frames) and the server's sanitiser. This inventory pins the limits so that the threat
// model cannot drift from the rule: if you close one, move its snippet to BAD above and update that row.
describe("what the lint cannot see (known limits; the layer that holds is named in docs/threat-model.md)", () => {
  it.each([
    ["the global object under another name", "const w = window; w.fetch('/x');", "connect-src 'self'"],
    ["a global read out of the global object by destructuring", "const { fetch: f } = window; f('/x');", "connect-src 'self'"],
    ["a sink reached through a name built at run time", "const name = ['inner', 'HTML'].join(''); el[name] = x;", "Trusted Types"],
    ["Function reached through a constructor", "(() => undefined).constructor('return 1')();", "no 'unsafe-eval'"],
    ["a module imported from a variable", "const m = await import(url);", "script-src 'self'"],
    ["a javascript: URL put in an href at run time", "link.setAttribute('href', value);", "script-src without 'unsafe-inline'"],
  ])("does not flag %s (held by: %s)", async (_name, code) => {
    expect(ids(await lint(`${code}\n`)), code).toEqual([]);
  });
});

describe("the named exceptions are as narrow as they say", () => {
  const SAMPLE = {
    network: "fetch('/api/v1/x');",
    "create-policy": "trustedTypes.createPolicy('x', {});",
    "web-storage": "localStorage.getItem('a');",
    "cookie-write": "document.cookie = 'a=b';",
    "global-patch": "Element.prototype.setAttribute = f;",
    worker: "new Worker(url);",
    "optimistic-update": "client.setQueryData(key, 1);",
  };
  const FILES = {
    "editor-workers": "src/editor/entry.ts",
    "api-client": "src/api/client.ts",
    "sse-client": "src/live/sse-client.ts",
    "editor-warm-up": "src/editor/warm.ts",
    "trusted-types-module": "src/lib/csp/trusted-types.ts",
    "prefs-module": "src/lib/prefs/index.ts",
    "monaco-global-patches": "src/editor/global-patches.ts",
    "optimistic-helper": "src/lib/query/optimistic.ts",
  };

  it("names a file for every exception", () => {
    expect(Object.keys(FILES).sort()).toEqual(EXCEPTIONS.map((e) => e.name).sort());
  });

  for (const exception of EXCEPTIONS) {
    it(`${exception.name}: allows ${exception.allow.join(", ")} in its file and nothing else`, async () => {
      const file = FILES[exception.name];
      expect(exception.why.length, "an exception explains itself").toBeGreaterThan(40);
      for (const id of exception.allow) expect(ids(await lint(`${SAMPLE[id]}\n`, file)), `${id} in ${file}`).not.toContain(id);
      // Everything else is still refused in that file, even the things other exceptions allow.
      for (const [id, code] of Object.entries(SAMPLE)) {
        if (exception.allow.includes(id)) continue;
        expect(ids(await lint(`${code}\n`, file)), `${id} in ${file}`).toContain(id);
      }
      expect(ids(await lint("el.innerHTML = x;\n", file))).toContain("inner-html");
      expect(ids(await lint("el.setAttribute('style', 'top:0');\n", file))).toContain("set-attribute-style");
    });
  }

  it("lifts the scope-aware forms of a restriction with the same exception, and only there", async () => {
    // Each exception that lifts a restriction lifts every way of writing it in its file (the global object, an alias, a computed name)...
    const FORMS = {
      "src/api/client.ts": ["await globalThis.fetch(u);", "const f = fetch;", "window['fetch'](u);"],
      "src/live/sse-client.ts": ["globalThis.fetch(u);", "const impl = options.fetch ?? fetch;"],
      "src/lib/prefs/index.ts": ["window['localStorage'].clear();", "const s = sessionStorage;", "caches.delete('x');", "cookieStore.delete('a');"],
    };
    for (const [file, snippets] of Object.entries(FORMS)) {
      for (const code of snippets) expect(ids(await lint(`${code}\n`, file)).filter((id) => id === "network" || id === "web-storage" || id === "cookie-write"), `${code} in ${file}`).toEqual([]);
    }
    // ...and none of them anywhere else.
    for (const [file, snippets] of Object.entries(FORMS)) {
      for (const code of snippets) expect(ids(await lint(`${code}\n`, "src/features/x/other.ts")).length, `${code} outside ${file}`).toBeGreaterThan(0);
    }
  });

  it("keeps the same restrictions on the neighbours of an exception file", async () => {
    expect(ids(await lint("fetch(u);\n", "src/api/other.ts"))).toContain("network");
    expect(ids(await lint("localStorage.getItem('a');\n", "src/features/prefs.ts"))).toContain("web-storage");
    expect(ids(await lint("Element.prototype.x = f;\n", "src/editor/other.ts"))).toContain("global-patch");
  });
});

describe("optimistic updates: the ban fires everywhere, and one file is exempt", () => {
  const ON_MUTATE = "export const m = (client) => ({ onMutate: (v) => client.setQueryData(key, v) });\n";

  it.each([
    "src/features/swimlanes/mutations.ts",
    "src/features/file/mutations.ts",
    "src/lib/query/write.ts",
    "src/lib/query/invalidate.ts",
    "src/lib/query/cursor-pages.ts",
    "src/lib/query/optimistic-helper.ts",
    "src/lib/query/optimistic/index.ts",
    "src/lib/query/optimistic.tsx",
    "src/lib/query/other/optimistic.ts",
    "src/lib/optimistic.ts",
    "src/live/apply.ts",
    "src/components/app/Save.tsx",
    "src/api/client.ts",
    "src/editor/loader.ts",
  ])("still refuses onMutate and setQueryData in %s", async (file) => {
    const found = ids(await lint(ON_MUTATE, file));
    expect(found.filter((id) => id === "optimistic-update")).toHaveLength(2);
  });

  it("allows both in src/lib/query/optimistic.ts, and only there", async () => {
    expect(ids(await lint(ON_MUTATE, "src/lib/query/optimistic.ts"))).not.toContain("optimistic-update");
    const exempt = EXCEPTIONS.filter((e) => e.allow.includes("optimistic-update"));
    expect(exempt.map((e) => e.files)).toEqual([["src/lib/query/optimistic.ts"]]);
  });

  it("does not let the helper's file do anything else the rules ban", async () => {
    for (const code of ["fetch('/api/v1/x');", "localStorage.getItem('a');", "el.innerHTML = x;", "client.invalidateQueries();", "useQuery({ queryKey: ['a'] });"]) {
      expect(ids(await lint(`${code}\n`, "src/lib/query/optimistic.ts")).length, code).toBeGreaterThan(0);
    }
  });

  it("the exception says what it is for", () => {
    const e = EXCEPTIONS.find((x) => x.name === "optimistic-helper");
    expect(e?.why).toMatch(/closed registry/);
    expect(e?.why).toMatch(/ALWAYS refetches/);
  });
});

describe("query keys and invalidation are written one way", () => {
  it.each([
    "client.invalidateQueries({ queryKey: swimlaneKeys.detail(id) });",
    "client.invalidateQueries({ predicate: (q) => keep(q) });",
    "client.invalidateQueries({ predicate, refetchType: 'active' });",
    "client.cancelQueries({ queryKey: key, exact: true });",
    "client.removeQueries({ queryKey: compareKeys.diffs() });",
    "useQuery({ queryKey: swimlaneKeys.all(), queryFn: f });",
    "useQuery({ queryKey, queryFn: f });",
    "client.getQueryData(swimlaneKeys.all());",
    "client.clear();",
    "const key = ['not', 'a', 'query', 'key'];",
    "import { listSwimlanes } from '@/api/generated/endpoints/swimlanes/swimlanes';",
  ])("passes %s", async (code) => {
    const found = ids(await lint(`${code}\n`));
    expect(found.filter((id) => id === "query-key-literal" || id === "unfiltered-invalidation"), code).toEqual([]);
  });

  it("does not apply to tests, mocks and the generated client", async () => {
    for (const file of ["src/lib/query/x.test.ts", "src/test/x.tsx", "src/mocks/x.ts"]) {
      expect(ids(await lint("client.invalidateQueries(); useQuery({ queryKey: ['a'] });\n", file)), file).toEqual([]);
    }
  });

  it("says where the keys are, in the message", async () => {
    const [m] = (await lint("useQuery({ queryKey: ['a'] });\n")).filter((x) => LK_RULES.has(x.ruleId));
    expect(m?.message).toMatch(/src\/lib\/query\/keys\.ts/);
  });
});

describe("plugin rules and the inline-config ban", () => {
  it("bans dangerouslySetInnerHTML everywhere but the one sink", async () => {
    const code = "export const A = ({ h }) => <div dangerouslySetInnerHTML={{ __html: h }} />;\n";
    expect(rules(await lint(code, PRODUCT))).toContain("react/no-danger");
    expect(rules(await lint(code, "src/components/app/Other.tsx"))).toContain("react/no-danger");
    expect(rules(await lint(code, "src/components/app/SanitisedDocHtml.tsx"))).not.toContain("react/no-danger");
  });

  it.each([
    ["react/jsx-no-target-blank", "export const A = () => <a href='https://x.example' target='_blank'>x</a>;\n"],
    ["react/jsx-no-script-url", "export const A = () => <a href='javascript:alert(1)'>x</a>;\n"],
    ["react/iframe-missing-sandbox", "export const A = () => <iframe src='/x' />;\n"],
    ["jsx-a11y/alt-text", "export const A = () => <img src='/x.png' />;\n"],
    ["react-hooks/rules-of-hooks", "export function A({ c }) { if (c) { useState(1); } return null; }\n"],
    ["no-console", "console.log('x');\n"],
    ["no-debugger", "debugger;\n"],
  ])("%s", async (rule, code) => {
    expect(rules(await lint(code)), code).toContain(rule);
  });

  it("does not let an eslint-disable comment turn a rule off inside src", async () => {
    const messages = await lint("// eslint-disable-next-line no-console\nconsole.log('x');\n");
    expect(rules(messages)).toContain("no-console");
    expect(messages.some((m) => /disable/i.test(m.message) && /inline|ignored|not allowed/i.test(m.message))).toBe(true);
  });

  it("reports a disable directive that disables nothing", async () => {
    const [result] = await eslint.lintText("// eslint-disable-next-line no-console\nexport const x = 1;\n", { filePath: join(WEB_ROOT, "scripts/x.mjs") });
    expect(result.messages.length).toBeGreaterThan(0);
  });
});
