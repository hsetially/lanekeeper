import { randomBytes } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { checkBundle, FAIL_KB, FORBIDDEN_PACKAGES, WARN_KB } from "./check-bundle.mjs";

// P10: the initial JavaScript is under 250 KB gzipped. The gate sums the gzip size (level 9) of the entry chunk and of every
// chunk it imports statically, taking 1 KB as 1,000 bytes. It fails above 250, warns above 225, and fails when a package that
// belongs in a lazy chunk or a test (Monaco, msw, faker, cmdk, dompurify, marked) is in that closure, whatever the size.
// Random bytes do not compress, so a file of N random bytes is N gzipped bytes, plus a few dozen of header.

let root;
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "lk-bundle-"));
});
afterEach(() => rmSync(root, { recursive: true, force: true }));

/** A dist with chunks of the given sizes in random bytes. `initial` are the entry and its static imports. */
function makeDist({ initial, lazy = {}, modules = {} }) {
  const dist = join(root, "dist");
  mkdirSync(join(dist, "assets"), { recursive: true });
  mkdirSync(join(dist, ".vite"), { recursive: true });
  const manifest = {};
  const names = Object.keys(initial);
  const [entryName, ...rest] = names;
  for (const [name, bytes] of Object.entries({ ...initial, ...lazy })) writeFileSync(join(dist, "assets", name), randomBytes(bytes));
  manifest["index.html"] = { file: `assets/${entryName}`, src: "index.html", isEntry: true, imports: rest.map((n) => `_${n}`) };
  for (const n of rest) manifest[`_${n}`] = { file: `assets/${n}` };
  for (const n of Object.keys(lazy)) manifest[`src/${n}`] = { file: `assets/${n}`, isDynamicEntry: true };
  writeFileSync(join(dist, ".vite", "manifest.json"), JSON.stringify(manifest));
  const modulesFile = join(root, "modules.json");
  writeFileSync(modulesFile, JSON.stringify(Object.fromEntries(Object.entries(modules).concat(Object.keys({ ...initial, ...lazy }).filter((n) => !(`assets/${n}` in modules)).map((n) => [`assets/${n}`, ["/w/src/main.tsx"]])))));
  return { dist, modulesFile };
}
const resultsDir = () => join(root, "results");

describe("the sizes", () => {
  it("counts a kilobyte as 1,000 bytes and fails above 250", () => {
    expect([FAIL_KB, WARN_KB]).toEqual([250, 225]);
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 251_500 } });
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(out.kb).toBeGreaterThan(251);
    expect(out.errors.join("\n")).toMatch(/over the 250 KB budget/);
  });

  it("passes under the budget, and sums the entry with everything it imports statically", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 60_000, "react-B.js": 70_000, "router-C.js": 30_000 } });
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(out.errors).toEqual([]);
    expect(out.files).toHaveLength(3);
    expect(out.kb).toBeGreaterThan(160);
    expect(out.kb).toBeLessThan(162);
  });

  it("does not count a lazy chunk, however big", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 80_000 }, lazy: { "editor-E.js": 3_000_000 } });
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(out.errors).toEqual([]);
    expect(out.kb).toBeLessThan(81);
  });

  it("warns above 225 and under 250, and still passes", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 230_000 } });
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(out.errors).toEqual([]);
    expect(out.warnings.join("\n")).toMatch(/above 225 KB/);
  });

  it("gzips what it counts: text that compresses is counted compressed", () => {
    const dist = join(root, "dist");
    mkdirSync(join(dist, "assets"), { recursive: true });
    mkdirSync(join(dist, ".vite"), { recursive: true });
    writeFileSync(join(dist, "assets", "index-A.js"), "const a = 1;\n".repeat(100_000));
    writeFileSync(join(dist, ".vite", "manifest.json"), JSON.stringify({ "index.html": { file: "assets/index-A.js", isEntry: true, src: "index.html" } }));
    const modulesFile = join(root, "modules.json");
    writeFileSync(modulesFile, JSON.stringify({ "assets/index-A.js": ["/w/src/main.tsx"] }));
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(out.rawBytes).toBe(1_300_000);
    expect(out.kb).toBeLessThan(10);
  });
});

describe("what must not be in the first screen", () => {
  it("names the packages, one by one", () => {
    expect([...FORBIDDEN_PACKAGES].sort()).toEqual(["@faker-js/faker", "cmdk", "dompurify", "marked", "monaco-editor", "msw"]);
  });

  it.each([
    ["monaco-editor", "/w/node_modules/.pnpm/monaco-editor@0.57.0/node_modules/monaco-editor/esm/vs/editor/editor.api.js"],
    ["msw", "/w/node_modules/.pnpm/msw@3.0.1_x/node_modules/msw/lib/index.mjs"],
    ["@faker-js/faker", "/w/node_modules/.pnpm/@faker-js+faker@10.6.0/node_modules/@faker-js/faker/dist/index.js"],
    ["cmdk", "/w/node_modules/.pnpm/cmdk@1.1.1_x/node_modules/cmdk/dist/index.mjs"],
    ["dompurify", "/w/node_modules/.pnpm/dompurify@3.4.16/node_modules/dompurify/dist/purify.es.mjs"],
    ["marked", "/w/node_modules/.pnpm/marked@14.0.0/node_modules/marked/lib/marked.esm.js"],
  ])("fails when %s is in the initial closure, even when the size is small", (name, id) => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 10_000, "vendor-B.js": 10_000 }, modules: { "assets/vendor-B.js": [id, "/w/src/x.ts"] } });
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(out.errors.join("\n")).toContain(name);
    expect(out.errors.join("\n")).toContain("vendor-B.js");
  });

  it("lets the same packages be in a lazy chunk", () => {
    const { dist, modulesFile } = makeDist({
      initial: { "index-A.js": 10_000 },
      lazy: { "entry-E.js": 20_000 },
      modules: { "assets/entry-E.js": ["/w/node_modules/.pnpm/monaco-editor@0.57.0/node_modules/monaco-editor/esm/vs/editor/editor.api.js"] },
    });
    expect(checkBundle(dist, { modulesFile, resultsDir: resultsDir() }).errors).toEqual([]);
  });

  it("does not take a package with a similar name for a banned one", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 10_000 }, modules: { "assets/index-A.js": ["/w/node_modules/.pnpm/marked-highlight@2.0.0/node_modules/marked-highlight/index.js", "/w/node_modules/.pnpm/mswjs@1/node_modules/mswjs/x.js", "/w/src/marked.ts"] } });
    expect(checkBundle(dist, { modulesFile, resultsDir: resultsDir() }).errors).toEqual([]);
  });
});

describe("it never passes by not looking", () => {
  it("fails when there is no dist", () => {
    expect(checkBundle(join(root, "nothing"), { modulesFile: join(root, "m.json"), resultsDir: resultsDir() }).errors.join("\n")).toMatch(/does not exist/);
  });

  it("fails when there is no manifest", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 1_000 } });
    rmSync(join(dist, ".vite"), { recursive: true });
    expect(checkBundle(dist, { modulesFile, resultsDir: resultsDir() }).errors.join("\n")).toMatch(/manifest/);
  });

  it("fails when the list of modules is missing, because the packages cannot be checked", () => {
    const { dist } = makeDist({ initial: { "index-A.js": 1_000 } });
    expect(checkBundle(dist, { modulesFile: join(root, "absent.json"), resultsDir: resultsDir() }).errors.join("\n")).toMatch(/list of modules/);
  });

  it("fails when a file the manifest names is not there", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 1_000 } });
    rmSync(join(dist, "assets", "index-A.js"));
    expect(checkBundle(dist, { modulesFile, resultsDir: resultsDir() }).errors.length).toBeGreaterThan(0);
  });

  it("fails when a chunk of the first screen has no entry in the list of modules", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 1_000 } });
    writeFileSync(modulesFile, "{}");
    expect(checkBundle(dist, { modulesFile, resultsDir: resultsDir() }).errors.join("\n")).toMatch(/index-A\.js/);
  });
});

describe("the result", () => {
  it("is written for the checker: kilobytes, the max statistic, one value", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 50_000 } });
    const out = checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    const file = JSON.parse(readFileSync(join(resultsDir(), "web-initial_js_gzip_kb.json"), "utf8"));
    expect(file).toMatchObject({ bench: "web/initial_js_gzip_kb", unit: "kb", statistic: "max" });
    expect(file.value).toBeCloseTo(out.kb, 6);
    expect(file.files).toHaveLength(1);
  });

  it("is not written when the gate fails to look, so a stale number cannot stand in", () => {
    const { dist } = makeDist({ initial: { "index-A.js": 1_000 } });
    checkBundle(dist, { modulesFile: join(root, "absent.json"), resultsDir: resultsDir() });
    expect(() => readFileSync(join(resultsDir(), "web-initial_js_gzip_kb.json"), "utf8")).toThrow();
  });

  it("is written even over the budget, so the checker reports the number", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 260_000 } });
    checkBundle(dist, { modulesFile, resultsDir: resultsDir() });
    expect(JSON.parse(readFileSync(join(resultsDir(), "web-initial_js_gzip_kb.json"), "utf8")).value).toBeGreaterThan(250);
  });

  it("is not written when asked not to (the perf build)", () => {
    const { dist, modulesFile } = makeDist({ initial: { "index-A.js": 1_000 } });
    checkBundle(dist, { modulesFile, resultsDir: resultsDir(), writeResult: false });
    expect(() => readFileSync(join(resultsDir(), "web-initial_js_gzip_kb.json"), "utf8")).toThrow();
  });
});
