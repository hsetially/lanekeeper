import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { chunkHints, editorWarmFiles, WARM_META_NAME } from "./chunk-hints.mjs";

// The editor is a lazy chunk of a few megabytes. For an Editor and above, once the file view is up, its files are fetched
// into the HTTP cache while the browser is idle (src/editor/warm.ts), but never imported. The page cannot know the hashed
// names, so the build writes them into index.html, in a <meta> element (data, not script), after the bundle exists.

const chunk = (fileName, extra = {}) => ({ type: "chunk", fileName, imports: [], dynamicImports: [], isEntry: false, viteMetadata: { importedCss: new Set() }, ...extra });
const asset = (fileName) => ({ type: "asset", fileName });

function bundle() {
  return {
    "assets/index-APP.js": chunk("assets/index-APP.js", { isEntry: true, facadeModuleId: "/w/index.html" }),
    "assets/vendor-V.js": chunk("assets/vendor-V.js"),
    "assets/entry-ED.js": chunk("assets/entry-ED.js", {
      facadeModuleId: "/w/src/editor/entry.ts",
      isDynamicEntry: true,
      imports: ["assets/index-APP.js", "assets/editor-core-C.js"],
      viteMetadata: { importedCss: new Set(["assets/entry-ED.css"]) },
    }),
    "assets/editor-core-C.js": chunk("assets/editor-core-C.js", { imports: ["assets/vendor-V.js"] }),
    "assets/yaml-L.js": chunk("assets/yaml-L.js", { facadeModuleId: "/w/node_modules/monaco-editor/esm/vs/languages/definitions/yaml/yaml.js" }),
    "assets/editor.worker-WK.js": asset("assets/editor.worker-WK.js"),
    "assets/Inter-400-F.woff2": asset("assets/Inter-400-F.woff2"),
  };
}

describe("editorWarmFiles", () => {
  it("lists the editor chunk, what it needs that the first screen did not already load, its styles and its worker", () => {
    expect(editorWarmFiles(bundle(), { editorEntry: "src/editor/entry.ts" }).sort()).toEqual(
      ["/assets/editor-core-C.js", "/assets/editor.worker-WK.js", "/assets/entry-ED.css", "/assets/entry-ED.js", "/assets/vendor-V.js"].sort(),
    );
  });

  it("leaves out the files of the first screen: they are already in the cache", () => {
    const files = editorWarmFiles(bundle(), { editorEntry: "src/editor/entry.ts" });
    expect(files).not.toContain("/assets/index-APP.js");
  });

  it("is empty when no screen loads the editor (a build without Edit mode has no editor chunk)", () => {
    const b = bundle();
    delete b["assets/entry-ED.js"];
    expect(editorWarmFiles(b, { editorEntry: "src/editor/entry.ts" })).toEqual([]);
  });

  it("does not follow a dynamic import: the language files load when the editor asks for them", () => {
    const b = bundle();
    b["assets/entry-ED.js"].dynamicImports = ["assets/yaml-L.js"];
    expect(editorWarmFiles(b, { editorEntry: "src/editor/entry.ts" })).not.toContain("/assets/yaml-L.js");
  });

  it("finds the chunk by the module it holds when Rollup names no facade for it (it does not, for the real editor entry)", () => {
    const b = bundle();
    b["assets/entry-ED.js"].facadeModuleId = null;
    b["assets/entry-ED.js"].modules = { "/w/node_modules/monaco-editor/esm/vs/editor/editor.api.js": {}, "/w/src/editor/entry.ts": {} };
    expect(editorWarmFiles(b, { editorEntry: "src/editor/entry.ts" })).toContain("/assets/entry-ED.js");
  });

  it("does not take a static chunk that merely contains the entry for the lazy editor chunk", () => {
    const b = bundle();
    b["assets/entry-ED.js"].isDynamicEntry = false;
    expect(editorWarmFiles(b, { editorEntry: "src/editor/entry.ts" })).toEqual([]);
  });

  it("matches the entry however the path is written (Windows separators)", () => {
    const b = bundle();
    b["assets/entry-ED.js"].facadeModuleId = "C:\\w\\src\\editor\\entry.ts";
    expect(editorWarmFiles(b, { editorEntry: "src/editor/entry.ts" })).toContain("/assets/entry-ED.js");
  });

  it("only ever names files under /assets/, whatever the bundle holds", () => {
    const b = bundle();
    b["assets/entry-ED.js"].imports.push("../outside.js");
    b["../outside.js"] = chunk("../outside.js");
    for (const file of editorWarmFiles(b, { editorEntry: "src/editor/entry.ts" })) expect(file).toMatch(/^\/assets\/[A-Za-z0-9._-]+\.(js|css)$/);
  });
});

describe("the plugin", () => {
  const run = (b) => {
    const plugin = chunkHints();
    const hook = plugin.transformIndexHtml;
    expect(hook.order).toBe("post");
    return hook.handler("<html><head></head><body></body></html>", { bundle: b, filename: "index.html" });
  };

  it("adds one <meta> element with the files, in the head", () => {
    const out = run(bundle());
    expect(out.tags).toHaveLength(1);
    const tag = out.tags[0];
    expect(tag).toMatchObject({ tag: "meta", injectTo: "head" });
    expect(tag.attrs.name).toBe(WARM_META_NAME);
    expect(tag.attrs.content.split(" ").sort()).toEqual(["/assets/editor-core-C.js", "/assets/editor.worker-WK.js", "/assets/entry-ED.css", "/assets/entry-ED.js", "/assets/vendor-V.js"]);
  });

  it("adds nothing when there is no editor chunk, and nothing in dev (no bundle)", () => {
    const b = bundle();
    delete b["assets/entry-ED.js"];
    expect(run(b).tags ?? []).toEqual([]);
    const plugin = chunkHints();
    expect(plugin.transformIndexHtml.handler("<html></html>", { filename: "index.html" })).toBeUndefined();
  });

  it("is a build plugin only", () => {
    expect(chunkHints().apply).toBe("build");
  });
});

describe("the name the page looks for", () => {
  it("is the name the build writes (the page's copy is in src/editor/warm.ts)", () => {
    const page = readFileSync(new URL("../../src/editor/warm.ts", import.meta.url), "utf8");
    expect(page).toContain(`export const WARM_META_NAME = "${WARM_META_NAME}"`);
  });

  it("is a name no other meta element of the page uses", () => {
    const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
    expect(html).not.toContain(WARM_META_NAME);
  });
});
