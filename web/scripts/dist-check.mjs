// Scans a built dist for what the hub's CSP and Trusted Types would refuse or what the app must not load (S12). It runs at
// the end of `pnpm build` (so also in CI and in the image build), never skips, and fails when dist/ is missing.
//
// It reads what the browser will load, not URL-shaped strings: it parses every JavaScript file and looks at what is passed
// to `import()`, `new Worker`, `fetch`, `importScripts`, `setAttribute`, and at `src`, `href`, `action`... properties set to
// an absolute URL; it parses HTML for inline script, inline style, handlers and off-site references; it reads CSS `url()`.
// A second, cheap net refuses any string that names a known CDN or font host. Source maps are not scanned (and must not exist
// in a production build). Known literal namespaces are an explicit allowlist below.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import * as acorn from "acorn";

/** URLs that appear in code as names, not requests, and the one link a person follows by hand. */
export const ALLOWED_URL_PREFIXES = [
  "http://www.w3.org/", // XML, SVG and MathML namespaces
  "https://react.dev/errors", // React's error decoder, printed in a message
  "https://reactjs.org/docs/error-decoder.html", // the same, older spelling
  // The one outside link of the app: where to create a fine-grained GitHub token. A navigation by the user
  // (rel="noopener noreferrer", opened in another tab), not a request; the page itself never loads it.
  "https://github.com/settings/personal-access-tokens",
];

/** Strings that name a host the CSP would block and a reviewer would never want. */
const FORBIDDEN_HOSTS = ["unpkg.com", "cdn.jsdelivr.net", "cdnjs.cloudflare.com", "fonts.googleapis.com", "fonts.gstatic.com", "ajax.googleapis.com", "esm.sh", "cdn.skypack.dev", "googletagmanager.com", "google-analytics.com", "sentry.io", "segment.com", "hotjar.com"];

/** Strings that must not be in any chunk: the Radix style injectors, which Trusted Types and the nonce refuse (F11). */
const FORBIDDEN_STRINGS = ["data-radix-select-viewport", "data-radix-scroll-area-viewport"];

/** Test-only routes exist in `--mode perf` builds and must not be in a production dist (checked unless `allowTestPages`). */
const TEST_PAGE_MARKER = "/testpages/";

const URL_PROPERTIES = new Set(["src", "href", "action", "data", "srcset", "poster", "formaction", "xlinkHref"]);
const ABSOLUTE = /^\s*(https?:)?\/\//i;
const SCRIPT_URL = /^\s*(javascript|vbscript):/i;
const NONCE_PLACEHOLDER = "__CSP_NONCE__";

/** @param {string} url */
function allowed(url) {
  return ALLOWED_URL_PREFIXES.some((prefix) => url.startsWith(prefix));
}

function* files(dir, base = dir) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) yield* files(full, base);
    else yield { path: full, rel: relative(base, full).split("\\").join("/") };
  }
}

/** Walk an ESTree without recursion (a minified chunk can nest thousands deep). */
function walk(root, visit) {
  const stack = [root];
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node || typeof node.type !== "string") continue;
    visit(node);
    for (const key of Object.keys(node)) {
      if (key === "type" || key === "start" || key === "end" || key === "loc" || key === "range") continue;
      const value = node[key];
      if (Array.isArray(value)) {
        for (let i = value.length - 1; i >= 0; i--) if (value[i] && typeof value[i].type === "string") stack.push(value[i]);
      } else if (value && typeof value.type === "string") stack.push(value);
    }
  }
}

const textOf = (node) => {
  if (!node) return undefined;
  if (node.type === "Literal" && typeof node.value === "string") return node.value;
  if (node.type === "TemplateLiteral" && node.quasis.length > 0) return node.quasis[0].value.cooked ?? undefined;
  return undefined;
};

const propertyName = (node) => (node.type === "Identifier" ? node.name : node.type === "Literal" && typeof node.value === "string" ? node.value : undefined);

/** @returns {{ errors: string[]; notes: string[] }} */
function checkJs(rel, source, options = {}) {
  const errors = [];
  const notes = [];
  let ast;
  try {
    ast = acorn.parse(source, { ecmaVersion: "latest", sourceType: "module", allowHashBang: true });
  } catch (error) {
    try {
      ast = acorn.parse(source, { ecmaVersion: "latest", sourceType: "script", allowHashBang: true });
    } catch {
      return { errors: [`${rel}: could not be parsed (${error.message})`], notes };
    }
  }
  const urlArg = (node, what) => {
    const text = textOf(node);
    if (text === undefined) return;
    if (SCRIPT_URL.test(text)) errors.push(`${rel}: ${what} with a script URL`);
    else if (ABSOLUTE.test(text) && !allowed(text)) errors.push(`${rel}: ${what} an off-site URL (${text.slice(0, 80)})`);
  };
  walk(ast, (node) => {
    switch (node.type) {
      case "CallExpression": {
        const callee = node.callee;
        if (callee.type === "Identifier") {
          if (callee.name === "eval") errors.push(`${rel}: eval() call`);
          if (callee.name === "Function") errors.push(`${rel}: Function() constructor call`);
          if (callee.name === "fetch" || callee.name === "importScripts") urlArg(node.arguments[0], `${callee.name}() with`);
          if ((callee.name === "setTimeout" || callee.name === "setInterval") && textOf(node.arguments[0]) !== undefined) errors.push(`${rel}: ${callee.name}() with a string, which is eval`);
        } else if (callee.type === "MemberExpression" && !callee.computed && callee.property.type === "Identifier") {
          const name = callee.property.name;
          if (name === "fetch") urlArg(node.arguments[0], "fetch() with");
          if (name === "setAttribute") {
            const attr = textOf(node.arguments[0])?.toLowerCase();
            if (attr && URL_PROPERTIES.has(attr)) urlArg(node.arguments[1], `setAttribute(${attr}) with`);
            if (attr === "style") notes.push(`${rel}: setAttribute('style') at ${node.start} (blocked by the CSP unless routed to CSSOM)`);
          }
          if (name === "open" && callee.object.type === "Identifier" && callee.object.name === "window") urlArg(node.arguments[0], "window.open() with");
        }
        break;
      }
      case "NewExpression": {
        const callee = node.callee;
        if (callee.type === "Identifier") {
          if (callee.name === "Function") errors.push(`${rel}: new Function()`);
          if (callee.name === "Worker" || callee.name === "SharedWorker" || callee.name === "EventSource" || callee.name === "WebSocket") urlArg(node.arguments[0], `new ${callee.name}() with`);
        }
        break;
      }
      case "ImportExpression": {
        const text = textOf(node.source);
        if (text === undefined) notes.push(`${rel}: import() with a computed specifier at ${node.start}`);
        else if (ABSOLUTE.test(text)) errors.push(`${rel}: import() of an off-site URL (${text.slice(0, 80)})`);
        break;
      }
      case "Property": {
        const key = node.key ? propertyName(node.key) : undefined;
        if (key && URL_PROPERTIES.has(key)) urlArg(node.value, `property ${key} set to`);
        break;
      }
      case "AssignmentExpression": {
        const left = node.left;
        if (left.type === "MemberExpression" && !left.computed && left.property.type === "Identifier" && URL_PROPERTIES.has(left.property.name)) urlArg(node.right, `.${left.property.name} assigned`);
        break;
      }
      case "Literal":
      case "TemplateElement": {
        const text = node.type === "Literal" ? (typeof node.value === "string" ? node.value : undefined) : (node.value.cooked ?? undefined);
        if (text !== undefined && text.length > 3) {
          for (const host of FORBIDDEN_HOSTS) if (text.includes(host)) errors.push(`${rel}: a string names ${host}`);
        }
        break;
      }
      default:
    }
  });
  for (const text of FORBIDDEN_STRINGS) if (source.includes(text)) errors.push(`${rel}: contains ${text} (a Radix <style> injector, refused by Trusted Types and the nonce)`);
  if (!options.allowTestPages && source.includes(TEST_PAGE_MARKER)) errors.push(`${rel}: mentions ${TEST_PAGE_MARKER}, a route that exists only in the perf-mode build`);
  return { errors, notes };
}

function checkCss(rel, source) {
  const errors = [];
  for (const match of source.matchAll(/url\(\s*(['"]?)([^'")]*)\1\s*\)/gi)) {
    const url = match[2] ?? "";
    if (ABSOLUTE.test(url)) errors.push(`${rel}: url() of an off-site URL (${url.slice(0, 80)})`);
  }
  for (const match of source.matchAll(/@import\s+(?:url\()?\s*['"]?([^'")\s;]+)/gi)) {
    const url = match[1] ?? "";
    if (ABSOLUTE.test(url)) errors.push(`${rel}: @import of an off-site URL (${url.slice(0, 80)})`);
  }
  for (const block of source.matchAll(/@font-face\s*\{[^}]*\}/gi)) {
    if (/url\(\s*['"]?data:/i.test(block[0])) errors.push(`${rel}: a font is a data: URL (font-src allows 'self' only)`);
  }
  for (const text of FORBIDDEN_STRINGS) if (source.includes(text)) errors.push(`${rel}: contains ${text}`);
  for (const host of FORBIDDEN_HOSTS) if (source.includes(host)) errors.push(`${rel}: names ${host}`);
  return errors;
}

function checkHtml(rel, source) {
  const errors = [];
  if (!source.includes(NONCE_PLACEHOLDER)) errors.push(`${rel}: has no ${NONCE_PLACEHOLDER} placeholder (the hub refuses to start without it)`);
  if (!/<meta\s+property="csp-nonce"/i.test(source)) errors.push(`${rel}: has no <meta property="csp-nonce">`);
  const tags = [...source.matchAll(/<(script|link|style)\b([^>]*)>([\s\S]*?)(?=<)/gi)];
  for (const [, tag, attrs, inner] of tags) {
    const name = tag.toLowerCase();
    if (name === "style") errors.push(`${rel}: an inline <style> element`);
    if (name === "script" && !/\bsrc\s*=/.test(attrs ?? "")) errors.push(`${rel}: an inline <script>`);
    if (name === "script" && (inner ?? "").trim() !== "" && /\bsrc\s*=/.test(attrs ?? "")) errors.push(`${rel}: a <script src> with a body`);
    const nonce = /\bnonce\s*=\s*"([^"]*)"/.exec(attrs ?? "");
    const isStylesheet = name === "link" && /rel\s*=\s*"(stylesheet|modulepreload|preload)"/i.test(attrs ?? "");
    if ((name === "script" && /\bsrc\s*=/.test(attrs ?? "") && /type\s*=\s*"module"/.test(attrs ?? "")) || isStylesheet) {
      if (!nonce || nonce[1] !== NONCE_PLACEHOLDER) errors.push(`${rel}: a <${name}> without nonce="${NONCE_PLACEHOLDER}"`);
    }
  }
  if (/\sstyle\s*=/i.test(source)) errors.push(`${rel}: a style attribute`);
  if (/\son[a-z]+\s*=/i.test(source.replace(/<!--[\s\S]*?-->/g, ""))) errors.push(`${rel}: an inline event handler attribute`);
  for (const match of source.matchAll(/\b(src|href|action)\s*=\s*"([^"]*)"/gi)) {
    const value = match[2] ?? "";
    if (value.startsWith("data:image/")) continue; // the page icon; img-src allows data:
    if (ABSOLUTE.test(value) || SCRIPT_URL.test(value) || value.startsWith("data:")) {
      if (!allowed(value)) errors.push(`${rel}: ${match[1]}="${value.slice(0, 60)}" is not a path on this origin`);
    }
  }
  for (const host of FORBIDDEN_HOSTS) if (source.includes(host)) errors.push(`${rel}: names ${host}`);
  return errors;
}

/**
 * @param {string} dist
 * @param {{ allowTestPages?: boolean }} [options]
 * @returns {{ errors: string[]; notes: string[]; scanned: number }}
 */
export function checkDist(dist, options = {}) {
  const errors = [];
  const notes = [];
  if (!existsSync(dist) || !statSync(dist).isDirectory()) return { errors: [`${dist} does not exist: run the build first (this check never skips)`], notes, scanned: 0 };
  if (!existsSync(join(dist, "index.html"))) errors.push("index.html is missing");
  if (!existsSync(join(dist, ".vite", "manifest.json"))) errors.push(".vite/manifest.json is missing (the bundle gate needs it)");
  let scanned = 0;
  for (const { path, rel } of files(dist)) {
    if (rel.endsWith(".map")) {
      if (process.env["LK_SOURCEMAP"] !== "1") errors.push(`${rel}: a source map in a production build (set LK_SOURCEMAP=1 only for local debugging)`);
      continue;
    }
    if (rel.startsWith(".vite/")) continue;
    if (/\.(m?js|cjs)$/.test(rel)) {
      const result = checkJs(rel, readFileSync(path, "utf8"), options);
      errors.push(...result.errors);
      notes.push(...result.notes);
      scanned++;
    } else if (rel.endsWith(".css")) {
      errors.push(...checkCss(rel, readFileSync(path, "utf8")));
      scanned++;
    } else if (rel.endsWith(".html")) {
      errors.push(...checkHtml(rel, readFileSync(path, "utf8")));
      scanned++;
    }
  }
  return { errors, notes, scanned };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const dist = process.argv.slice(2).find((a) => !a.startsWith("--")) ?? fileURLToPath(new URL("../dist", import.meta.url));
  const { errors, notes, scanned } = checkDist(dist, { allowTestPages: process.argv.includes("--allow-test-pages") });
  for (const note of notes) process.stdout.write(`note: ${note}\n`);
  if (errors.length > 0) {
    process.stderr.write(`dist-check failed (${errors.length}):\n${errors.map((e) => `  - ${e}`).join("\n")}\n`);
    process.exit(1);
  }
  process.stdout.write(`dist-check: ${scanned} files scanned, nothing the CSP would refuse\n`);
}
