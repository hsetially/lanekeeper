// The names of the editor's files, for the page (P10). The editor is a lazy chunk of a few megabytes, loaded only by Edit
// mode. For an Editor and above, once the file view is up, its files are fetched into the HTTP cache while the browser is idle
// (src/editor/warm.ts) so that Edit mode opens at once; they are never imported, because evaluating Monaco is the expensive
// part. The page cannot know the hashed file names, so the build writes them, after the bundle exists, into a <meta>
// element of index.html (data, not script, so the CSP has nothing to say). A build with no editor chunk writes nothing.

export const WARM_META_NAME = "lk-warm-editor";

const norm = (path) => path.split("\\").join("/");
// Only files of the app's own /assets/, the shape the hub serves and src/editor/warm.ts accepts.
const SAFE_FILE = /^assets\/[A-Za-z0-9._-]+\.(js|css)$/;

/** The file names of `entry`'s static imports, itself included, following `imports` and not `dynamicImports`. */
function closure(bundle, entry) {
  const seen = new Set();
  const stack = [entry];
  while (stack.length > 0) {
    const file = stack.pop();
    if (seen.has(file)) continue;
    seen.add(file);
    const item = bundle[file];
    if (item?.type === "chunk") for (const dep of item.imports ?? []) stack.push(dep);
  }
  return seen;
}

/**
 * @param {Record<string, any>} bundle the Rollup bundle: file name to chunk or asset
 * @param {{ editorEntry: string; workerPattern?: RegExp }} options
 * @returns {string[]} URL paths, `/assets/...`, of the editor's JavaScript, its styles and its worker
 */
export function editorWarmFiles(bundle, { editorEntry, workerPattern = /(^|\/)editor\.worker-[A-Za-z0-9_-]+\.js$/ }) {
  const wanted = `/${norm(editorEntry)}`;
  // Rollup names a chunk's `facadeModuleId` only when one module is its face; the editor's entry is a bare list of imports
  // and re-exports that Rollup merges with what it pulls in, so the chunk is found by the module it contains.
  const holds = (item) => (typeof item.facadeModuleId === "string" && norm(item.facadeModuleId).endsWith(wanted)) || Object.keys(item.modules ?? {}).some((id) => norm(id).endsWith(wanted));
  const editor = Object.values(bundle).find((item) => item.type === "chunk" && item.isDynamicEntry === true && holds(item));
  if (!editor) return [];

  // What the first screen already loaded is in the cache; fetching it again would only waste the idle time.
  const initial = new Set();
  for (const item of Object.values(bundle)) if (item.type === "chunk" && item.isEntry) for (const file of closure(bundle, item.fileName)) initial.add(file);

  const files = new Set();
  for (const file of closure(bundle, editor.fileName)) {
    if (initial.has(file)) continue;
    const item = bundle[file];
    if (item?.type !== "chunk") continue;
    files.add(file);
    for (const css of item.viteMetadata?.importedCss ?? []) files.add(css);
  }
  for (const item of Object.values(bundle)) if (item.type === "asset" && workerPattern.test(item.fileName)) files.add(item.fileName);

  return [...files].filter((file) => SAFE_FILE.test(file)).map((file) => `/${file}`);
}

/**
 * @param {{ editorEntry?: string }} [options]
 * @returns {import("vite").Plugin}
 */
export function chunkHints({ editorEntry = "src/editor/entry.ts" } = {}) {
  return {
    name: "lanekeeper-chunk-hints",
    apply: "build",
    transformIndexHtml: {
      order: "post",
      handler(_html, ctx) {
        if (!ctx.bundle) return undefined;
        const files = editorWarmFiles(ctx.bundle, { editorEntry });
        if (files.length === 0) return { html: _html, tags: [] };
        return { html: _html, tags: [{ tag: "meta", attrs: { name: WARM_META_NAME, content: files.join(" ") }, injectTo: "head" }] };
      },
    },
  };
}
