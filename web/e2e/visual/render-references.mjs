// Renders the design handoff's frames so that the app can be compared with them (the plan's "References are rendered, not
// exported"). The handoff is a set of `.dc.html` files that a small runtime (`support.js`) turns into pages with React 18; this
// file opens them in the browser the app runs in, from a throwaway static server that serves `design/handoff/screens` READ-ONLY,
// with none of the app's code and no CSP, then hands back a screenshot of the frame.
//
//  - React 18.3.1: support.js asks unpkg.com for the two UMD files. The sandbox cannot reach unpkg, and the plan's dev
//    dependency for them (W30) is not approved, so the files are given to this module as a directory (`LK_REACT18_DIR`,
//    the two files of `npm pack react@18.3.1 react-dom@18.3.1`). Their SHA-384 is compared with the SRI that support.js carries
//    for the same URLs, so the bytes are the ones the design was made with. Without the directory nothing can be rendered, and
//    `reactUmd()` says so: a spec that needs a reference reports "NOT RUN", it never passes without one.
//  - The theme: a dark frame has `data-theme="dark"` put on <html>, and the `theme` prop default of the Shell and Access Screen
//    files is rewritten in memory. Before the screenshot the frame's own `--background` is read and must be the token of the
//    theme that was asked for, so a light frame can never be taken for a dark one.
//  - The clock is frozen and animations are off.
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));
export const SCREENS_DIR = resolve(WEB_ROOT, "..", "design", "handoff", "screens");

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".ttf": "font/ttf",
  ".json": "application/json",
  ".md": "text/plain; charset=utf-8",
};

/** The background token of each theme (design/handoff/screens/tokens/colors.css). */
export const BACKGROUND = { light: "#ffffff", dark: "#0e1116" };

/** The frozen clock of the visual runs (a fixed instant, so that "4 min ago" is the same every time). */
export const FROZEN_NOW = 1_760_000_000_000;

const SRI = {
  "react.production.min.js": /REACT_SRI = "(sha384-[^"]+)"/,
  "react-dom.production.min.js": /REACT_DOM_SRI = "(sha384-[^"]+)"/,
};

/**
 * The two UMD files, as text, once their hashes have been checked against support.js. `{ ok: false, reason }` when the
 * directory is not given or does not hold them; throws when a file is not the one the design asks for.
 */
export function reactUmd(dir = process.env["LK_REACT18_DIR"]) {
  if (!dir) return { ok: false, reason: "LK_REACT18_DIR is not set (the directory holding react.production.min.js and react-dom.production.min.js of React 18.3.1)" };
  const support = readFileSync(join(SCREENS_DIR, "support.js"), "utf8");
  const files = {};
  for (const [name, pattern] of Object.entries(SRI)) {
    const path = join(dir, name);
    if (!existsSync(path)) return { ok: false, reason: `${path} does not exist` };
    const bytes = readFileSync(path);
    const wanted = pattern.exec(support)?.[1];
    const got = `sha384-${createHash("sha384").update(bytes).digest("base64")}`;
    if (wanted === undefined || got !== wanted) throw new Error(`${path} is not the file support.js asks for (${got} is not ${String(wanted)})`);
    files[name] = bytes.toString("utf8");
  }
  return { ok: true, react: files["react.production.min.js"], reactDom: files["react-dom.production.min.js"] };
}

/** The sha-256 of a file, hex, for the manifest. */
export function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/** Put a `theme` default into the `data-props` of a `.dc.html` page, in memory. */
export function withThemeDefault(html, theme) {
  return html.replace(/data-props="([^"]*)"/, (whole, attr) => {
    const json = attr.replaceAll("&quot;", '"').replaceAll("&amp;", "&");
    let props;
    try {
      props = JSON.parse(json);
    } catch {
      return whole;
    }
    if (props.theme && typeof props.theme === "object") props.theme.default = theme;
    return `data-props="${JSON.stringify(props).replaceAll("&", "&amp;").replaceAll('"', "&quot;")}"`;
  });
}

/** Serve the handoff's screens directory, read only, from 127.0.0.1 on a free port. */
export async function startReferenceServer({ themeFor } = {}) {
  const root = SCREENS_DIR + sep;
  const server = createServer((req, res) => {
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.writeHead(405).end();
      return;
    }
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const path = resolve(SCREENS_DIR, normalize(decodeURIComponent(url.pathname)).replace(/^[/\\]+/, ""));
    if (!path.startsWith(root) || !existsSync(path)) {
      res.writeHead(404).end();
      return;
    }
    let body = readFileSync(path);
    if (path.endsWith(".dc.html")) {
      const theme = themeFor?.(url);
      if (theme) body = Buffer.from(withThemeDefault(body.toString("utf8"), theme));
    }
    res.writeHead(200, { "content-type": TYPES[extname(path)] ?? "application/octet-stream", "cache-control": "no-store" });
    res.end(req.method === "HEAD" ? undefined : body);
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const address = server.address();
  const origin = `http://127.0.0.1:${typeof address === "object" && address ? address.port : 0}`;
  return {
    origin,
    close: () => new Promise((done) => server.close(() => done())),
  };
}

/**
 * Open one frame in `page` and wait until it is drawn. `file` is the `.dc.html` name, `theme` is `light` or `dark`, `viewport`
 * is the size of the frame. `omit` lists selectors of elements the app deliberately does not have, each with a row in
 * design/DEVIATIONS.md; they are removed from the reference so that the rest of the row lays out as the app's does. Returns when
 * the frame has rendered and the theme token is the one asked for.
 */
export async function openReference(page, server, umd, /** @type {{ file: string; theme: "light" | "dark"; viewport: { width: number; height: number }; omit?: string[] }} */ { file, theme, viewport, omit = [] }) {
  if (!umd.ok) throw new Error(`cannot render a reference: ${umd.reason}`);
  await page.setViewportSize(viewport);
  // Date.now() is fixed; timers still run, so the runtime's own timeouts are not held up.
  await page.clock.setFixedTime(FROZEN_NOW);
  await page.addInitScript(
    ({ react, reactDom, dark }) => {
      // support.js uses window.React and window.ReactDOM when they are there, and asks unpkg when they are not.
      (0, eval)(react);
      (0, eval)(reactDom);
      if (dark) document.documentElement.setAttribute("data-theme", "dark");
    },
    { react: umd.react, reactDom: umd.reactDom, dark: theme === "dark" },
  );
  await page.goto(`${server.origin}/${encodeURIComponent(file)}?theme=${theme}`);
  await page.waitForSelector("aside[aria-label='Primary']", { state: "visible", timeout: 20_000 });
  // A standalone page has the browser's 8 px body margin, and its buttons use the browser's own font (the app's reset makes
  // them inherit, as the design's host page does); the board the frame is taken from places it flush in a card.
  await page.addStyleTag({ content: "html,body{margin:0!important}button{font:inherit}*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}" });
  for (const selector of omit) await page.evaluate((sel) => document.querySelectorAll(sel).forEach((el) => el.remove()), selector);
  await page.evaluate(() => document.fonts.ready);
  const token = await page.evaluate(() => {
    const frame = document.querySelector("[data-theme]");
    return (frame ? getComputedStyle(frame) : getComputedStyle(document.documentElement)).getPropertyValue("--background").trim().toLowerCase();
  });
  if (token !== BACKGROUND[theme]) throw new Error(`the ${theme} frame of ${file} has --background ${token}, not ${BACKGROUND[theme]}`);
}
