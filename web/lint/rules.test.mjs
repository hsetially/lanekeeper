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
const ids = (messages) => messages.filter((m) => m.ruleId === "no-restricted-syntax").map((m) => /^\[lk\/([a-z-]+)\]/.exec(m.message)?.[1]);
const rules = (messages) => messages.map((m) => m.ruleId);

/** id -> snippets that must each produce that id. */
const BAD = {
  "inner-html": ["el.innerHTML = x;", "el.outerHTML = '<b>';", "el['innerHTML'] = x;", "document.body.innerHTML += x;"],
  "html-parsers": ["el.insertAdjacentHTML('beforeend', x);", "range.createContextualFragment(x);", "new DOMParser().parseFromString(x, 'text/html');", "document.write(x);", "document.writeln(x);"],
  srcdoc: ["frame.srcdoc = x;", "const a = <iframe srcDoc={x} sandbox='' />;"],
  "dangerous-elements": ["document.createElement('script');", "document.createElement('IFRAME');", "document.createElement('object');", "document.createElement('embed');"],
  "event-handler-property": ["el.onclick = f;", "window.onerror = f;", "el.setAttribute('onload', 'x()');"],
  "set-attribute-style": ["el.setAttribute('style', 'top:0');", "el.setAttribute('STYLE', x);"],
  "javascript-url": ["const u = 'javascript:alert(1)';", "const u = ' JavaScript:alert(1)';", "const u = `javascript:${x}`;"],
  eval: ["eval(x);", "new Function('return 1');", "Function('return 1')();"],
  navigation: ["window.open(x);", "location.href = x;", "window.location = x;", "window.location.href = x;", "location = x;", "location.assign(x);", "window.location.replace(x);"],
  "dynamic-regexp": ["new RegExp(q);", "RegExp(q, 'i');", "new RegExp(`^${q}`);"],
  "create-policy": ["window.trustedTypes.createPolicy('x', {});", "trustedTypes.createPolicy('x', {});"],
  "web-storage": ["localStorage.setItem('a', 'b');", "window.sessionStorage.clear();", "indexedDB.open('x');"],
  "cookie-write": ["document.cookie = 'a=b';"],
  "form-submit": ["const f = <form method='post'><button>x</button></form>;", "const f = <form action='/x' />;"],
  "optimistic-update": ["useMutation({ onMutate: () => {} });", "client.setQueryData(['a'], 1);"],
  network: ["fetch('/api/v1/x');", "fetch(`/api/v1/${id}`);", "window.fetch(u);", "globalThis.fetch(u);", "new XMLHttpRequest();", "new WebSocket(u);", "new EventSource(u);", "navigator.sendBeacon(u, d);"],
  worker: ["new Worker(url);", "new SharedWorker(url);", "URL.createObjectURL(blob);", "navigator.serviceWorker.register(url);"],
  "global-patch": ["Element.prototype.setAttribute = f;", "Document.prototype.createElement = f;", "Object.defineProperty(HTMLElement.prototype, 'x', {});"],
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
    const [m] = (await lint("el.innerHTML = x;\n")).filter((x) => x.ruleId === "no-restricted-syntax");
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

describe("the named exceptions are as narrow as they say", () => {
  const SAMPLE = {
    network: "fetch('/api/v1/x');",
    "create-policy": "trustedTypes.createPolicy('x', {});",
    "web-storage": "localStorage.getItem('a');",
    "cookie-write": "document.cookie = 'a=b';",
    "global-patch": "Element.prototype.setAttribute = f;",
    worker: "new Worker(url);",
  };
  const FILES = {
    "editor-workers": "src/editor/entry.ts",
    "api-client": "src/api/client.ts",
    "sse-client": "src/live/sse-client.ts",
    "editor-warm-up": "src/editor/warm.ts",
    "trusted-types-module": "src/lib/csp/trusted-types.ts",
    "prefs-module": "src/lib/prefs/index.ts",
    "monaco-global-patches": "src/editor/global-patches.ts",
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

  it("keeps the same restrictions on the neighbours of an exception file", async () => {
    expect(ids(await lint("fetch(u);\n", "src/api/other.ts"))).toContain("network");
    expect(ids(await lint("localStorage.getItem('a');\n", "src/features/prefs.ts"))).toContain("web-storage");
    expect(ids(await lint("Element.prototype.x = f;\n", "src/editor/other.ts"))).toContain("global-patch");
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
