// The bundle gate (P10): the initial JavaScript is under 250 KB gzipped. It runs at the end of `pnpm build`, so CI, the image
// build and a developer's build all run it, and it never passes by not looking.
//
// What counts: the entry chunk and every chunk it imports statically (Vite's manifest, `imports`, not `dynamicImports`),
// each file gzipped at level 9, summed, 1 KB taken as 1,000 bytes. It fails above 250, warns above 225, and fails when a
// package that belongs in a lazy chunk or a test (Monaco, msw, faker, cmdk, dompurify, marked) is in that closure, whatever
// the size: a package like that in the first screen is the next regression, and the size gate would only see it late.
//
// Which modules a chunk holds comes from the build itself (scripts/build/bundle-report.mjs writes it next to the build, not
// into dist). With no manifest or no module list the gate FAILS: it cannot tell, and "cannot tell" is not "fine".
//
// The number goes to target/perf-results/web-initial_js_gzip_kb.json for `cargo xtask bench-check` (budget
// P10.initial_js_gzip_kb). The hub sends no Content-Encoding (plan F1, W28), so this is the size of the files as the page
// would get them if the load balancer compresses; as served raw they are about three times that.

import { existsSync, readFileSync, statSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipBytes, initialClosure } from "../build/bundle-report.mjs";
import { resultsDir as defaultResultsDir, writeResult } from "./results.mjs";

export const FAIL_KB = 250;
export const WARN_KB = 225;
export const FORBIDDEN_PACKAGES = ["monaco-editor", "msw", "@faker-js/faker", "cmdk", "dompurify", "marked"];
// The live stream engine (the parser, the client, the plan of what an event invalidates) is a chunk of its own, imported by
// src/live/LiveProvider.tsx when a signed-in user with a role is on screen. The sign-in path never needs it, so it must not
// be in the first screen's JavaScript; one static import of it from anywhere in the entry would put it back.
export const LAZY_APP_MODULES = ["src/live/live.ts", "src/live/sse-client.ts", "src/live/sse-parser.ts", "src/live/events.ts", "src/live/invalidation.ts", "src/live/apply.ts"];

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));
const escape = (name) => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const FORBIDDEN = new RegExp(`(?:^|/)node_modules/(${FORBIDDEN_PACKAGES.map(escape).join("|")})/`);

/** Where the build writes the module list for a dist. */
export const modulesFileFor = (dist) => join(WEB_ROOT, ".build-report", `${basename(resolve(dist))}.modules.json`);

/**
 * @param {string} dist
 * @param {{ modulesFile?: string; resultsDir?: string; writeResult?: boolean }} [options]
 */
export function checkBundle(dist, { modulesFile = modulesFileFor(dist), resultsDir = defaultResultsDir(), writeResult: write = true } = {}) {
  const errors = [];
  const warnings = [];
  const fail = (reason) => ({ ok: false, errors: [...errors, reason], warnings, kb: Number.NaN, files: [], rawBytes: 0, entry: "" });

  if (!existsSync(dist) || !statSync(dist).isDirectory()) return fail(`${dist} does not exist: run the build first (this gate never skips)`);
  const manifestPath = join(dist, ".vite", "manifest.json");
  if (!existsSync(manifestPath)) return fail(`${manifestPath} is missing: the build must write Vite's manifest (build.manifest)`);
  if (!existsSync(modulesFile)) return fail(`the list of modules of the build (${modulesFile}) is missing: scripts/build/bundle-report.mjs writes it, and without it the packages in the first screen cannot be checked`);

  let manifest;
  let modules;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    modules = JSON.parse(readFileSync(modulesFile, "utf8"));
  } catch (error) {
    return fail(`could not read the manifest or the list of modules: ${error.message}`);
  }

  let closure;
  try {
    closure = initialClosure(manifest);
  } catch (error) {
    return fail(error.message);
  }

  const files = [];
  for (const file of closure.files) {
    let buffer;
    try {
      buffer = readFileSync(join(dist, file));
    } catch {
      errors.push(`${file} is in the manifest and not in ${dist}`);
      continue;
    }
    files.push({ file, bytes: buffer.length, gzip: gzipBytes(buffer) });
    const ids = modules[file];
    if (!Array.isArray(ids)) {
      errors.push(`${file} has no entry in the list of modules, so its packages cannot be checked`);
      continue;
    }
    const found = new Set();
    for (const id of ids) {
      const match = FORBIDDEN.exec(String(id).split("\\").join("/"));
      if (match?.[1]) found.add(match[1]);
    }
    for (const name of found) errors.push(`${file}, in the initial JavaScript, contains ${name}: it belongs in a lazy chunk (or in a test), never in the first screen`);
    const eager = new Set();
    for (const id of ids) {
      const normal = String(id).split("\\").join("/");
      for (const lazy of LAZY_APP_MODULES) if (normal.endsWith(`/${lazy}`)) eager.add(lazy);
    }
    for (const name of eager) errors.push(`${file}, in the initial JavaScript, contains ${name}: the live stream engine is loaded when a signed-in user is on screen, never on the sign-in path (import it with import() from src/live/LiveProvider.tsx only)`);
  }

  const gzip = files.reduce((n, f) => n + f.gzip, 0);
  const rawBytes = files.reduce((n, f) => n + f.bytes, 0);
  const kb = gzip / 1000;
  if (kb > FAIL_KB) errors.push(`the initial JavaScript is ${kb.toFixed(1)} KB gzipped, over the ${FAIL_KB} KB budget (P10)`);
  else if (kb > WARN_KB) warnings.push(`the initial JavaScript is ${kb.toFixed(1)} KB gzipped, above ${WARN_KB} KB: the budget is ${FAIL_KB} KB (cut order: palette and shortcuts sheet to lazy, icons to route chunks, rarely used Radix parts to their routes)`);

  // Written whatever the size, so that the checker reports the number it judges. Not written when the gate could not look.
  if (write && files.length > 0) {
    writeResult({ bench: "web/initial_js_gzip_kb", unit: "kb", statistic: "max", value: kb, rawKb: rawBytes / 1000, files: files.map((f) => ({ file: f.file, gzipKb: f.gzip / 1000 })), dist: basename(resolve(dist)) }, { dir: resultsDir });
  }
  return { ok: errors.length === 0, errors, warnings, kb, files, rawBytes, entry: closure.entry };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const args = process.argv.slice(2);
  const dist = resolve(WEB_ROOT, args.find((a) => !a.startsWith("--")) ?? "dist");
  const result = checkBundle(dist, { writeResult: !args.includes("--no-result") });
  for (const f of result.files) process.stdout.write(`  ${f.file}  ${(f.gzip / 1000).toFixed(1)} KB gzip  (${(f.bytes / 1000).toFixed(0)} KB raw)\n`);
  if (Number.isFinite(result.kb)) process.stdout.write(`bundle gate: initial JavaScript ${result.kb.toFixed(1)} KB gzipped of ${FAIL_KB} (${(result.rawBytes / 1000).toFixed(0)} KB raw)\n`);
  for (const w of result.warnings) process.stdout.write(`warning: ${w}\n`);
  if (!result.ok) {
    process.stderr.write(`bundle gate failed (${result.errors.length}):\n${result.errors.map((e) => `  - ${e}`).join("\n")}\n`);
    process.exit(1);
  }
}
