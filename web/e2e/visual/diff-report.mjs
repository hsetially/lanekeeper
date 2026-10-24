// WHERE a comparison with the design differs, in words. A tier-1 test that goes over its limit used to say how many pixels differ
// ("8 of 1134"); the first run of the baseline bootstrap on GitHub failed two of them by a hair on a Chromium that is not the one the
// margins were measured on, and a count cannot tell a shift of one pixel from a different font from a different layout. This reads the
// pictures that were compared and prints a short text (under 40 lines):
//
//  - the pixels that differ, grouped into CLUSTERS: cells of 16 px that have one, merged when they touch (also by a corner), at most
//    eight, the largest first. Each has its box in CSS pixels (the pictures are taken at a device scale of 1, so a pixel is a CSS pixel),
//    its count and its share of the differences, and the mean absolute difference of the colour of its pixels (0 to 255);
//  - for each, what the two renderings look like there: the extent of the INK in the design and in the app (the thing the cluster
//    touches: the pixels that are not the background, joined when they are within 2 px of each other, looked for within 24 px of the
//    cluster), so that "the same size, one pixel to the right" is told from "a different size"; the shift of up to 2 px each way that
//    would make the two agree best, and how many pixels are left then; and the two pixels that differ most, with their colours;
//  - the element under the cluster's centre in the app (`document.elementFromPoint`: tag, role, name or text cut to 60 characters, the
//    `data-slot`, the box) and, where the design's page can be asked, in the design too, with whether the two boxes are the same size.
//    The centre of a box is often between two letters, where the point is the row's, so the point asked is the pixel that differs
//    NEAREST the centre: it is in the thing that is drawn differently.
//
// It reads the picture of the difference that `compareImages` made (a pixel that differs is pure red there) and the two pictures it
// was made from, so it works for every comparison that has them, with masks painted out or not. It never decides anything: a
// comparison that passes does not call it, and one that fails fails as before, with this text added. It never throws (a report that
// cannot be made says so in a line), and what it asks a page is bounded: eight points, text cut to 60 characters, and no text at all
// from a field, an editor, a value that is hidden or revealed, or a token (nothing that could be a secret goes into a log).
//
// The parts that need no browser (`clusterCells`, `trimName`, `describeElement`, `formatReport`) are read in scripts/diff-report.test.mjs;
// the parts that read pictures in e2e/visual/diff-report.spec.ts.

/** The cell of the grid, the number of clusters shown, and the longest name of an element. */
export const CELL = 16;
export const MAX_CLUSTERS = 8;
export const NAME_LIMIT = 60;

/** The rule of compare.mjs: 0.2 of the largest distance, per pixel. */
const THRESHOLD = 0.2;
/** How far, in pixels, the shift of the app is searched each way; how much of the picture around a cluster is read for its ink; the largest area read. */
const SHIFT = 2;
const WINDOW = 24;
const MAX_AREA = 600_000;
const MAX_LINES = 40;

/**
 * @typedef {{ cx: number, cy: number, count: number, sum: number, x0: number, y0: number, x1: number, y1: number }} Cell
 *   one cell of the grid that has pixels that differ: its place, how many, the sum of their colour differences, and the box of those pixels (inclusive)
 * @typedef {{ x0: number, y0: number, x1: number, y1: number, count: number, sum: number, cells: number }} Cluster
 * @typedef {{ w: number, h: number, x: number, y: number }} Ink  the extent of what is not the background in a box, from the box's corner
 * @typedef {{ dx: number, dy: number, before: number, after: number }} Shift
 * @typedef {{ x: number, y: number, ref: string, app: string }} Worst
 * @typedef {{ ref?: Ink | null, app?: Ink | null, shift?: Shift | null, worst: Worst[], near?: { x: number, y: number } }} Measure  `near` is the pixel that differs nearest the cluster's centre
 * @typedef {{ tag: string, role: string, name: string, slot: string, cls: string, x: number, y: number, w: number, h: number }} Described  one element of a page, as the probe reads it
 */

// ----- without a browser ---------------------------------------------------------------------------------------------------------------

/**
 * Merge the cells that touch into clusters, largest first (a tie: the one nearer the top, then the left), and keep `limit` of them. What
 * is left out is counted.
 * @param {readonly Cell[]} cells
 * @param {number} [limit]
 * @returns {{ clusters: Cluster[], rest: { clusters: number, pixels: number } }}
 */
export function clusterCells(cells, limit = MAX_CLUSTERS) {
  const keep = Number.isInteger(limit) && limit > 0 ? limit : 0;
  const at = new Map(cells.map((cell, i) => [`${cell.cx},${cell.cy}`, i]));
  const parent = cells.map((_, i) => i);
  const find = (i) => {
    let root = i;
    while (parent[root] !== root) root = parent[root];
    // Path compression: every cell on the way points at the root.
    for (let walk = i; parent[walk] !== root; ) {
      const next = parent[walk];
      parent[walk] = root;
      walk = next;
    }
    return root;
  };
  for (const [i, cell] of cells.entries()) {
    for (let dy = -1; dy <= 1; dy += 1) {
      for (let dx = -1; dx <= 1; dx += 1) {
        const j = at.get(`${cell.cx + dx},${cell.cy + dy}`);
        if (j !== undefined && j !== i) parent[find(i)] = find(j);
      }
    }
  }
  /** @type {Map<number, Cluster>} */
  const groups = new Map();
  for (const [i, cell] of cells.entries()) {
    const root = find(i);
    const group = groups.get(root);
    if (group === undefined) groups.set(root, { x0: cell.x0, y0: cell.y0, x1: cell.x1, y1: cell.y1, count: cell.count, sum: cell.sum, cells: 1 });
    else {
      group.x0 = Math.min(group.x0, cell.x0);
      group.y0 = Math.min(group.y0, cell.y0);
      group.x1 = Math.max(group.x1, cell.x1);
      group.y1 = Math.max(group.y1, cell.y1);
      group.count += cell.count;
      group.sum += cell.sum;
      group.cells += 1;
    }
  }
  const all = [...groups.values()].sort((a, b) => b.count - a.count || a.y0 - b.y0 || a.x0 - b.x0);
  const left = all.slice(keep);
  return { clusters: all.slice(0, keep), rest: { clusters: left.length, pixels: left.reduce((n, c) => n + c.count, 0) } };
}

/** Text as one short line: control characters and runs of white space become one space, and at most `limit` characters are kept. */
export function trimName(text, limit = NAME_LIMIT) {
  const flat = Array.from(String(text ?? ""))
    .map((ch) => {
      const code = ch.charCodeAt(0);
      return code < 32 || (code >= 127 && code < 160) ? " " : ch;
    })
    .join("")
    .replace(/\s+/g, " ")
    .trim();
  return flat.length > limit ? `${flat.slice(0, Math.max(0, limit - 3))}...` : flat;
}

const num = (n) => String(Number(Number(n).toFixed(1)));
const signed = (n) => (n > 0 ? `+${n}` : String(n));
const box = (e) => `${num(e.w)}x${num(e.h)}`;

/** One element as one line: `<tag> role=r "name" slot=s WxH@(x,y)`, or `.class.class` where the page has no slot (the design has none). */
export function describeElement(element) {
  if (element === null || element === undefined) return "nothing at that point";
  const parts = [`<${trimName(element.tag, 20).replace(/[^a-z0-9-]/gi, "")}>`];
  if (element.role) parts.push(`role=${trimName(element.role, 30)}`);
  const name = trimName(element.name);
  if (name !== "") parts.push(`"${name}"`);
  if (element.slot) parts.push(`slot=${trimName(element.slot, 40)}`);
  else {
    const classes = trimName(element.cls, 200).split(" ").filter(Boolean).slice(0, 2);
    if (classes.length > 0) parts.push(classes.map((c) => `.${c.slice(0, 30)}`).join(""));
  }
  parts.push(`${box(element)}@(${num(element.x)},${num(element.y)})`);
  return parts.join(" ");
}

const pct = (n, of) => (of > 0 ? ((n / of) * 100).toFixed(1) : "0.0");
const inkText = (ink) => (ink === undefined ? "?" : ink === null ? "none" : `${ink.w}x${ink.h}`);

/** What the two renderings look like in a cluster's box, in one line. */
function measureLine(m) {
  const parts = [];
  if (m.ref !== undefined || m.app !== undefined) {
    let verdict;
    if (m.ref && m.app) {
      verdict = m.ref.w === m.app.w && m.ref.h === m.app.h ? "same size" : "sizes differ";
      const dx = m.app.x - m.ref.x;
      const dy = m.app.y - m.ref.y;
      if (verdict === "same size" && (dx !== 0 || dy !== 0)) verdict += `, the app's is at (${signed(dx)},${signed(dy)})`;
    } else verdict = m.ref || m.app ? "ink on one side only" : "no ink on either side";
    parts.push(`ink ${inkText(m.ref)} | ${inkText(m.app)}: ${verdict}`);
  } else parts.push("ink not measured (the area is too large)");
  if (m.shift) {
    parts.push(m.shift.dx === 0 && m.shift.dy === 0 ? `no shift of up to ${SHIFT} px agrees better (${m.shift.before} px)` : `best shift of the app (${signed(m.shift.dx)},${signed(m.shift.dy)}): ${m.shift.before} -> ${m.shift.after} px`);
  }
  if (m.worst.length > 0) parts.push(`worst ${m.worst.map((w) => `(${w.x},${w.y}) ${w.ref} -> ${w.app}`).join("; ")}`);
  return parts.join("; ");
}

/**
 * The report, as text. `clusters` are `Cluster`s with, each, `measure`, and `app` and `design` (a `Described`, `null` for nothing there,
 * `undefined` for not asked). `found` is the number of pixels in the picture of the difference, `differing` the number the caller counted
 * (more when parts that were compared apart, the `aligned` ones, differ).
 * @param {{ label: string, compared: number, differing: number, found: number, ratio: number, limit: number, clusters: Array<Cluster & { measure?: Measure, app?: Described | null, design?: Described | null }>, rest: { clusters: number, pixels: number }, notes?: readonly string[] }} r
 */
export function formatReport(r) {
  const lines = [];
  const more = r.differing - r.found;
  if (r.found === 0) {
    lines.push(`${r.label}: ${r.differing === 0 ? "no differing pixels" : "no differing pixels in the picture of the difference"} (of ${r.compared} compared)`);
    if (more > 0) lines.push(`${more} px more are in the aligned parts, which the picture of the difference does not hold`);
    return lines.join("\n");
  }
  const shown = r.clusters.length + r.rest.clusters;
  lines.push(`${r.label}: ${r.differing} of ${r.compared} compared px differ = ${(r.ratio * 100).toFixed(3)}% (limit ${(r.limit * 100).toFixed(3)}%)`);
  lines.push(`${shown} cluster${shown === 1 ? "" : "s"} of the differing pixels (cells of ${CELL} px merged when they touch), largest first; x and y are CSS px from the picture's top-left corner; an element is the one under the pixel that differs nearest the cluster's centre`);
  for (const note of (r.notes ?? []).slice(0, 3)) lines.push(trimName(note, 240));
  for (const [i, c] of r.clusters.entries()) {
    const w = c.x1 - c.x0 + 1;
    const h = c.y1 - c.y0 + 1;
    lines.push(`#${i + 1} x ${c.x0}..${c.x1} y ${c.y0}..${c.y1} (${w}x${h}) ${c.count} px = ${pct(c.count, r.found)}% of the differing pixels, mean abs diff ${Math.round(c.sum / Math.max(1, c.count))}/255`);
    if (c.measure) lines.push(`   ${measureLine(c.measure)}`);
    if (c.app !== undefined) lines.push(`   app    ${describeElement(c.app)}`);
    if (c.design !== undefined) {
      let same = "";
      if (c.app && c.design) same = c.app.w === c.design.w && c.app.h === c.design.h ? "; box is the same size as the app's" : `; box differs from the app's (${box(c.design)} against ${box(c.app)})`;
      lines.push(`   design ${describeElement(c.design)}${same}`);
    }
  }
  if (r.rest.clusters > 0) lines.push(`+ ${r.rest.clusters} more cluster${r.rest.clusters === 1 ? "" : "s"}, ${r.rest.pixels} px (${pct(r.rest.pixels, r.found)}%)`);
  if (more > 0) lines.push(`${more} px more are in the aligned parts, which the picture of the difference does not hold`);
  return lines.slice(0, MAX_LINES).join("\n");
}

// ----- in the browser ------------------------------------------------------------------------------------------------------------------

/**
 * The cells of the picture of the difference that have a pixel that differs (red there), with the colour difference of those pixels.
 * `page` only decodes the pictures: any page, best one that is not the app's.
 * @param {import("@playwright/test").Page} page
 * @param {{ reference: Buffer, app: Buffer, diffPng: string }} pictures `diffPng` is the data URL that compareImages returns
 * @returns {Promise<{ width: number, height: number, found: number, cells: Cell[] }>}
 */
export async function scanDifferences(page, pictures, cell = CELL) {
  return page.evaluate(
    async ({ a64, b64, d64, size }) => {
      const load = async (base64) => {
        const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const context = canvas.getContext("2d");
        context.drawImage(bitmap, 0, 0);
        return context.getImageData(0, 0, bitmap.width, bitmap.height);
      };
      const [ref, app, diff] = await Promise.all([load(a64), load(b64), load(d64)]);
      if (ref.width !== app.width || ref.height !== app.height || ref.width !== diff.width || ref.height !== diff.height) throw new Error("the three pictures are not the same size");
      const columns = Math.ceil(ref.width / size);
      const cells = new Map();
      let found = 0;
      for (let y = 0; y < ref.height; y += 1) {
        for (let x = 0; x < ref.width; x += 1) {
          const i = (y * ref.width + x) * 4;
          // compareImages paints a pixel that differs pure red and any other one in a grey-ish white (its green is 191 or more).
          if (diff.data[i + 1] >= 64) continue;
          found += 1;
          const delta = (Math.abs(ref.data[i] - app.data[i]) + Math.abs(ref.data[i + 1] - app.data[i + 1]) + Math.abs(ref.data[i + 2] - app.data[i + 2])) / 3;
          const cx = Math.floor(x / size);
          const cy = Math.floor(y / size);
          const key = cy * columns + cx;
          const seen = cells.get(key);
          if (seen === undefined) cells.set(key, { cx, cy, count: 1, sum: delta, x0: x, y0: y, x1: x, y1: y });
          else {
            seen.count += 1;
            seen.sum += delta;
            seen.x0 = Math.min(seen.x0, x);
            seen.y0 = Math.min(seen.y0, y);
            seen.x1 = Math.max(seen.x1, x);
            seen.y1 = Math.max(seen.y1, y);
          }
        }
      }
      return { width: ref.width, height: ref.height, found, cells: [...cells.values()] };
    },
    { a64: pictures.reference.toString("base64"), b64: pictures.app.toString("base64"), d64: pictures.diffPng.split(",")[1] ?? "", size: cell },
  );
}

/**
 * What the two renderings look like in the box of each cluster: the ink of each, the best shift of the app, the two pixels that differ most.
 * @param {import("@playwright/test").Page} page
 * @param {{ reference: Buffer, app: Buffer }} pictures
 * @param {ReadonlyArray<{ x0: number, y0: number, x1: number, y1: number }>} boxes
 * @returns {Promise<Measure[]>}
 */
export async function measureClusters(page, pictures, boxes, threshold = THRESHOLD) {
  return page.evaluate(
    async ({ a64, b64, list, limit, shiftBy, windowBy, maxArea }) => {
      const load = async (base64) => {
        const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const context = canvas.getContext("2d");
        context.drawImage(bitmap, 0, 0);
        return context.getImageData(0, 0, bitmap.width, bitmap.height);
      };
      const [ref, app] = await Promise.all([load(a64), load(b64)]);
      const width = ref.width;
      const height = ref.height;
      // The rule of compare.mjs (pixelmatch's distance in YIQ space), for opaque pictures, which screenshots are.
      const maxDelta = 35215 * limit * limit;
      const yiq = (r1, g1, b1, r2, g2, b2) => {
        const rr = r1 - r2;
        const gg = g1 - g2;
        const bb = b1 - b2;
        const y1 = rr * 0.29889531 + gg * 0.58662247 + bb * 0.11448223;
        const i1 = rr * 0.59597799 - gg * 0.2741761 - bb * 0.32180189;
        const q1 = rr * 0.21147017 - gg * 0.52261711 + bb * 0.31114694;
        return 0.5053 * y1 * y1 + 0.299 * i1 * i1 + 0.1957 * q1 * q1;
      };
      const hex = (d, i) => `#${[d[i], d[i + 1], d[i + 2]].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
      // The ink the cluster touches: in the window, the pixels that are not the background (the colour most of the window has), joined when
      // they are within 2 px of each other, starting from the ink inside the cluster's box (grown by one pixel). Its extent, from the window's corner.
      const ink = (d, win, seed) => {
        const w = win.x1 - win.x0 + 1;
        const h = win.y1 - win.y0 + 1;
        const counts = new Map();
        for (let y = win.y0; y <= win.y1; y += 1) {
          for (let x = win.x0; x <= win.x1; x += 1) {
            const i = (y * width + x) * 4;
            const key = (d.data[i] << 16) | (d.data[i + 1] << 8) | d.data[i + 2];
            counts.set(key, (counts.get(key) ?? 0) + 1);
          }
        }
        let mode = 0;
        let best = -1;
        for (const [key, n] of counts) {
          if (n > best) {
            best = n;
            mode = key;
          }
        }
        const br = mode >> 16;
        const bg = (mode >> 8) & 255;
        const bb = mode & 255;
        const mask = new Uint8Array(w * h);
        for (let y = 0; y < h; y += 1) {
          for (let x = 0; x < w; x += 1) {
            const i = ((win.y0 + y) * width + win.x0 + x) * 4;
            if (yiq(d.data[i], d.data[i + 1], d.data[i + 2], br, bg, bb) > maxDelta) mask[y * w + x] = 1;
          }
        }
        const seen = new Uint8Array(w * h);
        const queue = new Int32Array(w * h);
        let head = 0;
        let tail = 0;
        for (let y = Math.max(win.y0, seed.y0 - 1); y <= Math.min(win.y1, seed.y1 + 1); y += 1) {
          for (let x = Math.max(win.x0, seed.x0 - 1); x <= Math.min(win.x1, seed.x1 + 1); x += 1) {
            const at = (y - win.y0) * w + (x - win.x0);
            if (mask[at] === 1 && seen[at] === 0) {
              seen[at] = 1;
              queue[tail] = at;
              tail += 1;
            }
          }
        }
        if (tail === 0) return null;
        let minX = w;
        let minY = h;
        let maxX = -1;
        let maxY = -1;
        while (head < tail) {
          const at = queue[head];
          head += 1;
          const x = at % w;
          const y = (at - x) / w;
          minX = Math.min(minX, x);
          minY = Math.min(minY, y);
          maxX = Math.max(maxX, x);
          maxY = Math.max(maxY, y);
          for (let ny = Math.max(0, y - 2); ny <= Math.min(h - 1, y + 2); ny += 1) {
            for (let nx = Math.max(0, x - 2); nx <= Math.min(w - 1, x + 2); nx += 1) {
              const next = ny * w + nx;
              if (mask[next] === 1 && seen[next] === 0) {
                seen[next] = 1;
                queue[tail] = next;
                tail += 1;
              }
            }
          }
        }
        return { w: maxX - minX + 1, h: maxY - minY + 1, x: minX, y: minY };
      };
      return list.map((b) => {
        /** @type {{ worst: object[] } & Record<string, unknown>} */
        const out = { worst: [] };
        // The two pixels of the box that differ most, and the one that differs nearest its centre.
        const top = [];
        const mx = (b.x0 + b.x1) / 2;
        const my = (b.y0 + b.y1) / 2;
        let nearest = Infinity;
        for (let y = b.y0; y <= b.y1; y += 1) {
          for (let x = b.x0; x <= b.x1; x += 1) {
            const i = (y * width + x) * 4;
            const delta = yiq(ref.data[i], ref.data[i + 1], ref.data[i + 2], app.data[i], app.data[i + 1], app.data[i + 2]);
            if (delta <= maxDelta) continue;
            const away = (x - mx) * (x - mx) + (y - my) * (y - my);
            if (away < nearest) {
              nearest = away;
              out.near = { x, y };
            }
            if (top.length < 2 || delta > top[top.length - 1].delta) {
              top.push({ delta, x, y, ref: hex(ref.data, i), app: hex(app.data, i) });
              top.sort((p, q) => q.delta - p.delta);
              top.length = Math.min(top.length, 2);
            }
          }
        }
        out.worst = top.map(({ x, y, ref: r, app: a }) => ({ x, y, ref: r, app: a }));
        const win = { x0: Math.max(0, b.x0 - windowBy), y0: Math.max(0, b.y0 - windowBy), x1: Math.min(width - 1, b.x1 + windowBy), y1: Math.min(height - 1, b.y1 + windowBy) };
        if ((win.x1 - win.x0 + 1) * (win.y1 - win.y0 + 1) <= maxArea) {
          out.ref = ink(ref, win, b);
          out.app = ink(app, win, b);
        }
        // The shift: the pixel (x, y) of the design against the pixel (x + dx, y + dy) of the app, over the box grown by the shift (kept inside the picture).
        const sx0 = Math.max(shiftBy, b.x0 - shiftBy);
        const sy0 = Math.max(shiftBy, b.y0 - shiftBy);
        const sx1 = Math.min(width - 1 - shiftBy, b.x1 + shiftBy);
        const sy1 = Math.min(height - 1 - shiftBy, b.y1 + shiftBy);
        if (sx1 < sx0 || sy1 < sy0) return out;
        const tries = [];
        for (let dy = -shiftBy; dy <= shiftBy; dy += 1) for (let dx = -shiftBy; dx <= shiftBy; dx += 1) tries.push({ dx, dy });
        // (0,0) first and the nearest first, so that a tie keeps the smaller move.
        tries.sort((p, q) => Math.abs(p.dx) + Math.abs(p.dy) - (Math.abs(q.dx) + Math.abs(q.dy)));
        let before = 0;
        let chosen = null;
        for (const t of tries) {
          let n = 0;
          for (let y = sy0; y <= sy1; y += 1) {
            for (let x = sx0; x <= sx1; x += 1) {
              const i = (y * width + x) * 4;
              const j = ((y + t.dy) * width + (x + t.dx)) * 4;
              if (yiq(ref.data[i], ref.data[i + 1], ref.data[i + 2], app.data[j], app.data[j + 1], app.data[j + 2]) > maxDelta) n += 1;
            }
          }
          if (t.dx === 0 && t.dy === 0) before = n;
          if (chosen === null || n < chosen.after) chosen = { dx: t.dx, dy: t.dy, after: n };
        }
        out.shift = chosen === null ? null : { dx: chosen.dx, dy: chosen.dy, before, after: chosen.after };
        return out;
      });
    },
    { a64: pictures.reference.toString("base64"), b64: pictures.app.toString("base64"), list: boxes.map((b) => ({ x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1 })), limit: threshold, shiftBy: SHIFT, windowBy: WINDOW, maxArea: MAX_AREA },
  );
}

/**
 * Ask a page what is at some points of a picture taken of it: returns a function that gives, for each point (in the picture's pixels,
 * moved by `offset` when the picture is a clip of the page), the element at that point. No text is read from a field, an editor, a hidden
 * or revealed value or a token.
 * @param {import("@playwright/test").Page} page
 * @param {{ x: number, y: number }} [offset] where the picture's corner is, in the page
 * @returns {(points: ReadonlyArray<{ x: number, y: number }>) => Promise<Array<Described | null>>}
 */
export function elementProbe(page, offset = { x: 0, y: 0 }) {
  return async (points) =>
    page.evaluate(
      (list) => {
        const IMPLICIT = { a: "link", button: "button", img: "img", nav: "navigation", main: "main", aside: "complementary", header: "banner", footer: "contentinfo", table: "table", ul: "list", ol: "list", li: "listitem", h1: "heading", h2: "heading", h3: "heading", h4: "heading", h5: "heading", h6: "heading", input: "textbox", textarea: "textbox", select: "combobox", dialog: "dialog" };
        const NO_TEXT = "input, textarea, select, option, script, style, [contenteditable], .monaco-editor, [data-slot$='-value'], [data-slot*='secret'], [data-slot*='token']";
        return list.map((p) => {
          const el = document.elementFromPoint(p.x, p.y);
          if (el === null) return null;
          const tag = el.tagName.toLowerCase();
          const attr = (name) => el.getAttribute(name) ?? "";
          let name = attr("aria-label");
          if (name === "" && attr("aria-labelledby") !== "") {
            name = attr("aria-labelledby")
              .split(/\s+/)
              .map((id) => document.getElementById(id)?.textContent ?? "")
              .join(" ");
          }
          if (name === "") name = attr("title") || attr("alt");
          if (name === "" && el.closest(NO_TEXT) === null) name = el.textContent ?? "";
          const r = el.getBoundingClientRect();
          return {
            tag,
            role: attr("role") || IMPLICIT[tag] || "",
            // Cut here as well as in the report: a page of text is not sent across to be thrown away.
            name: name.slice(0, 200),
            slot: el.closest("[data-slot]")?.getAttribute("data-slot") ?? "",
            cls: typeof el.className === "string" ? el.className : "",
            x: r.x,
            y: r.y,
            w: r.width,
            h: r.height,
          };
        });
      },
      points.map((p) => ({ x: p.x + offset.x + 0.5, y: p.y + offset.y + 0.5 })),
    );
}

/**
 * The report of one comparison that is over its limit. Never throws.
 * @param {import("@playwright/test").Page} page decodes the pictures (any page)
 * @param {{ reference: Buffer, app: Buffer, diffPng: string }} pictures the two pictures as compared (masks painted) and the picture of the difference
 * @param {{ label: string, ratio: number, limit: number, differing: number, compared: number, threshold?: number, notes?: readonly string[], probes?: { app?: ReturnType<typeof elementProbe>, design?: ReturnType<typeof elementProbe> } }} options
 */
export async function explainDifference(page, pictures, options) {
  const notes = [...(options.notes ?? [])];
  try {
    const scan = await scanDifferences(page, pictures);
    const { clusters, rest } = clusterCells(scan.cells);
    const measures = clusters.length > 0 ? await measureClusters(page, pictures, clusters, options.threshold ?? THRESHOLD) : [];
    const centres = clusters.map((c, i) => measures[i]?.near ?? { x: Math.floor((c.x0 + c.x1) / 2), y: Math.floor((c.y0 + c.y1) / 2) });
    const ask = async (side, probe) => {
      if (probe === undefined || clusters.length === 0) return undefined;
      try {
        return await Promise.resolve().then(() => probe(centres));
      } catch (error) {
        notes.push(`${side}: not queried (${trimName(error instanceof Error ? error.message : String(error), 80)})`);
        return undefined;
      }
    };
    const app = await ask("app", options.probes?.app);
    const design = await ask("design", options.probes?.design);
    return formatReport({
      label: options.label,
      compared: options.compared,
      differing: options.differing,
      found: scan.found,
      ratio: options.ratio,
      limit: options.limit,
      clusters: clusters.map((c, i) => ({ ...c, measure: measures[i], ...(app ? { app: app[i] ?? null } : {}), ...(design ? { design: design[i] ?? null } : {}) })),
      rest,
      notes,
    });
  } catch (error) {
    return `${options.label}: the report of where the pixels differ could not be made (${trimName(error instanceof Error ? error.message : String(error), 120)})`;
  }
}

/** `explainDifference`, and the text to the console as well, which is what a CI log holds. Returns the text, to go into the failure message. */
export async function reportDifference(page, pictures, options) {
  const text = await explainDifference(page, pictures, options);
  console.log(`\n${text}\n`);
  return text;
}
