import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initialClosure, measureInitialJs, modulesOf } from "./bundle-report.mjs";

const manifest = {
  "index.html": { file: "assets/index-a.js", src: "index.html", isEntry: true, imports: ["_react.js", "_router.js"], dynamicImports: ["src/routes/files.tsx"] },
  "_react.js": { file: "assets/react-b.js" },
  "_router.js": { file: "assets/router-c.js", imports: ["_react.js"] },
  "src/routes/files.tsx": { file: "assets/files-d.js", isDynamicEntry: true, imports: ["_react.js", "_monaco.js"] },
  "_monaco.js": { file: "assets/monaco-e.js" },
};

describe("initialClosure", () => {
  it("is the entry and what it imports statically, each once", () => {
    const { entry, files } = initialClosure(manifest);
    expect(entry).toBe("assets/index-a.js");
    expect(files.sort()).toEqual(["assets/index-a.js", "assets/react-b.js", "assets/router-c.js"]);
  });

  it("does not follow a dynamic import: a lazy route and Monaco stay out of the initial JavaScript", () => {
    const { files } = initialClosure(manifest);
    expect(files.join()).not.toMatch(/files-d|monaco-e/);
  });

  it("fails loudly without an entry and on a dangling import", () => {
    expect(() => initialClosure({ "x.js": { file: "x.js" } })).toThrow(/no entry/);
    expect(() => initialClosure({ "index.html": { file: "a.js", src: "index.html", isEntry: true, imports: ["gone.js"] } })).toThrow(/gone\.js/);
  });
});

describe("measureInitialJs", () => {
  it("gzips each file of the closure at level 9 and sums them", () => {
    const dist = mkdtempSync(join(tmpdir(), "lk-dist-"));
    mkdirSync(join(dist, ".vite"));
    mkdirSync(join(dist, "assets"));
    writeFileSync(join(dist, ".vite", "manifest.json"), JSON.stringify({ "index.html": { file: "assets/a.js", src: "index.html", isEntry: true } }));
    writeFileSync(join(dist, "assets", "a.js"), "x".repeat(10_000));
    const m = measureInitialJs(dist);
    expect(m.totalRaw).toBe(10_000);
    expect(m.totalGzip).toBeGreaterThan(0);
    expect(m.totalGzip).toBeLessThan(200);
  });
});

describe("modulesOf", () => {
  it("lists, for each chunk, the modules that went into it, for the bundle gate's check of what is in the first screen", () => {
    const bundle = {
      "assets/index-a.js": { type: "chunk", fileName: "assets/index-a.js", modules: { "/w/src/main.tsx": {}, "/w/node_modules/react/index.js": {} } },
      "assets/Inter.woff2": { type: "asset", fileName: "assets/Inter.woff2" },
      "assets/files-d.js": { type: "chunk", fileName: "assets/files-d.js", modules: {} },
    };
    expect(modulesOf(bundle)).toEqual({ "assets/index-a.js": ["/w/src/main.tsx", "/w/node_modules/react/index.js"], "assets/files-d.js": [] });
  });
});
