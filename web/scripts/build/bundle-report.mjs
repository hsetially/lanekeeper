// The initial JavaScript of the app: the entry chunk plus every chunk it imports statically (the closure the browser must
// fetch before the first screen), each file gzipped at level 9, summed. 1 KB is 1,000 bytes. The bundle gate of T2
// (scripts/perf/check-bundle.mjs) uses the same function and fails above 250 KB; T1 only reports, so the first measured
// number is visible from the first build.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

/**
 * @param {Record<string, { file: string; isEntry?: boolean; imports?: string[]; css?: string[] }>} manifest Vite's manifest
 * @returns {{ entry: string; files: string[] }} the entry chunk's file and the files of its static import closure
 */
export function initialClosure(manifest) {
  const entries = Object.entries(manifest).filter(([, chunk]) => chunk.isEntry === true && /(^|\/)index\.html$/.test(chunk.src ?? ""));
  const found = entries.length > 0 ? entries : Object.entries(manifest).filter(([, chunk]) => chunk.isEntry === true);
  const first = found[0];
  if (!first) throw new Error("the manifest has no entry chunk");
  const seen = new Set();
  const files = [];
  const visit = (key) => {
    if (seen.has(key)) return;
    seen.add(key);
    const chunk = manifest[key];
    if (!chunk) throw new Error(`the manifest has no chunk ${key}`);
    files.push(chunk.file);
    for (const dep of chunk.imports ?? []) visit(dep);
  };
  visit(first[0]);
  return { entry: first[1].file, files };
}

/** Gzip size in bytes of `text` at level 9. */
export function gzipBytes(buffer) {
  return gzipSync(buffer, { level: 9 }).length;
}

/**
 * @param {string} distDir
 * @returns {{ entry: string; files: Array<{ file: string; bytes: number; gzip: number }>; totalGzip: number; totalRaw: number }}
 */
export function measureInitialJs(distDir) {
  const manifest = JSON.parse(readFileSync(join(distDir, ".vite", "manifest.json"), "utf8"));
  const { entry, files } = initialClosure(manifest);
  const rows = files.map((file) => {
    const buffer = readFileSync(join(distDir, file));
    return { file, bytes: buffer.length, gzip: gzipBytes(buffer) };
  });
  return { entry, files: rows, totalGzip: rows.reduce((n, r) => n + r.gzip, 0), totalRaw: rows.reduce((n, r) => n + r.bytes, 0) };
}

/**
 * A Vite plugin that prints the number after the build. It never fails the build.
 * @returns {import("vite").Plugin}
 */
export function bundleReport() {
  let outDir = "dist";
  let silent = false;
  return {
    name: "lanekeeper-bundle-report",
    apply: "build",
    configResolved(config) {
      outDir = config.build.outDir;
      silent = config.logLevel === "silent";
    },
    closeBundle() {
      if (silent) return;
      try {
        const m = measureInitialJs(outDir);
        const kb = (m.totalGzip / 1000).toFixed(1);
        process.stdout.write(`\ninitial JavaScript: ${kb} KB gzip (${m.files.length} files, ${(m.totalRaw / 1000).toFixed(0)} KB raw), budget 250 KB\n`);
      } catch {
        // A build with no manifest (a worker build, a failed build) has nothing to report.
      }
    },
  };
}
