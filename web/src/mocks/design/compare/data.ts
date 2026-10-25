// The data of the Compare page of the design (`Lanekeeper Compare Page.dc.html`), made by the same arithmetic as the page's own
// script, so that the grid has the same 5,000 settings, in the same order, with the same values in the same swimlanes: three
// channels of 1,520 settings and 440 shared ones, six swimlanes (sit, sitb, sitc, presita, presitb, presitc), about three settings
// in ten differing in some swimlane (a tenant swimlane differs from the base in one setting in fourteen to twenty). The page generates this in the browser; here it is served the way the hub serves it:
// a page of the grid at a time (a keyset cursor), the comparison of two swimlanes, and the tree of a whole-swimlane comparison.
//
// Line numbers are not the page's: the page's lines advance by one to three, which would make the hunks of two settings overlap,
// and a comparison from the server has hunks that do not. Here a setting sits eight lines after the one before it in every
// swimlane (shifted a little by swimlane, as the page does), and its hunk is seven lines.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension and `import type` for types.
import type { Comparison, DiffHunk, DiffLine, Grid, GridCell, GridRow, SettingChange, SettingValue, TreeComparison, TreeCompareEntry } from "../../../api/generated/model/index.ts";

export const CORE_FILE = "tx-infinity-api/tx-infinity-core.yml";
const GRID_PAGE_MAX = 500;
const DEFAULT_LIMIT = 100;

type Lane = { n: string; tier: "SIT" | "PRESIT"; kind: "tenant" | "base"; rate: [number, number, number] };
export const DESIGN_LANES: readonly Lane[] = [
  { n: "sit", tier: "SIT", kind: "tenant", rate: [0.045, 0.006, 0.003] },
  { n: "sitb", tier: "SIT", kind: "tenant", rate: [0.07, 0.01, 0.004] },
  { n: "sitc", tier: "SIT", kind: "base", rate: [0, 0, 0] },
  { n: "presita", tier: "PRESIT", kind: "tenant", rate: [0.09, 0.02, 0.006] },
  { n: "presitb", tier: "PRESIT", kind: "tenant", rate: [0.05, 0.008, 0.004] },
  { n: "presitc", tier: "PRESIT", kind: "base", rate: [0, 0, 0] },
];
const SHIFT = [-12, -37, 0, 112, 8, 0];
const CHANNELS = ["remote-itm-teller", "atm-iso", "atm"];

type Spec = { s: string; g: string[]; k: Array<[string, "n" | "s" | "b", Array<string | number>?]> };
const CHSPEC: Spec[] = [
  { s: "limits", g: ["withdrawal", "deposit", "transfer", "balance-inquiry", "bill-pay", "cash-advance"], k: [["max", "n", [5000, 10000, 20000, 50000, 100000]], ["min", "n", [1, 5, 10, 20]], ["daily-max", "n", [20000, 50000, 100000, 250000]], ["per-txn-max", "n", [2000, 5000, 10000]], ["currency", "s", ["USD", "EUR", "GBP", "CAD", "MXN"]], ["velocity-window-sec", "n", [60, 300, 900, 3600]]] },
  { s: "timeouts", g: ["host", "dispenser", "card-reader", "pin-pad", "session", "printer"], k: [["connect-ms", "n", [500, 1000, 1500, 3000]], ["read-ms", "n", [2000, 5000, 10000, 30000]], ["idle-ms", "n", [15000, 30000, 60000]], ["retries", "n", [0, 1, 2, 3, 5]], ["backoff-ms", "n", [100, 250, 500, 1000]]] },
  { s: "routing", g: ["primary", "secondary", "fallback-1", "fallback-2"], k: [["host", "s", ["hsm-sit-01.itm.internal", "hsm-sit-02.itm.internal", "auth-gw.sit.internal", "iso-switch-b.sit.internal"]], ["port", "n", [8443, 9443, 7001, 5672]], ["tls", "b"], ["weight", "n", [10, 25, 50, 100]], ["priority", "n", [1, 2, 3, 4, 5]]] },
  { s: "receipts", g: ["customer", "merchant"], k: [["enabled", "b"], ["template", "s", ["receipt-v2", "receipt-v3", "receipt-compact"]], ["copies", "n", [1, 2]], ["footer", "s", ["Thank you", "Keep this receipt", "Call 1-800-555-0142"]]] },
  { s: "iso8583", g: ["field-2", "field-3", "field-4", "field-11", "field-37", "field-39", "field-41", "field-52"], k: [["length", "n", [8, 12, 16, 19]], ["encoding", "s", ["ASCII", "EBCDIC", "BCD"]], ["required", "b"], ["mask", "s", ["none", "pan-6-4", "full"]]] },
  { s: "features", g: ["cardless-withdrawal", "balance-after-withdrawal", "multi-currency", "quick-cash", "deposit-escrow"], k: [["enabled", "b"], ["rollout-pct", "n", [0, 10, 25, 50, 100]], ["min-app-version", "s", ["4.2.0", "4.3.1", "4.4.0", "5.0.0"]]] },
];
const SHSPEC: Spec[] = [
  { s: "pool", g: ["db", "host-conn", "cache"], k: [["max-size", "n", [20, 50, 100]], ["min-idle", "n", [2, 5, 10]], ["validate", "b"], ["timeout-ms", "n", [1000, 3000, 5000]]] },
  { s: "logging", g: ["audit", "app", "trace"], k: [["level", "s", ["INFO", "WARN", "DEBUG"]], ["retention-days", "n", [7, 14, 30, 90]], ["json", "b"]] },
  { s: "metrics", g: ["jvm", "http", "queue"], k: [["enabled", "b"], ["interval-sec", "n", [10, 15, 30, 60]], ["prefix", "s", ["tx.core", "tx.itm", "tx.sit"]]] },
];

const H = (a: number, b: number): number => {
  let x = Math.imul(a + 1, 374761393) ^ Math.imul(b + 7, 668265263);
  x = Math.imul(x ^ (x >>> 13), 1274126177);
  x ^= x >>> 16;
  return (x >>> 0) / 4294967296;
};

type Cell = { k: "v"; t: string } | { k: "a" } | { k: "e" };
export type DesignRow = { i: number; path: string; ch: string | null; kind: "n" | "s" | "b"; cells: Cell[]; differ: boolean };

let ROWS: DesignRow[] | null = null;
export function designRows(): readonly DesignRow[] {
  if (ROWS) return ROWS;
  const out: DesignRow[] = [];
  const mk = (ch: string | null, prefix: string, spec: Spec, group: string, k: Spec["k"][number]): void => {
    const i = out.length;
    const kind = k[1];
    const opts = kind === "b" ? ["true", "false"] : (k[2] ?? []).map(String);
    const bi = kind === "b" ? (H(i, 1) < 0.7 ? 0 : 1) : Math.floor(H(i, 1) * opts.length);
    const base = opts[bi] ?? "";
    const alt = opts[(bi + 1 + Math.floor(H(i, 2) * (opts.length - 1))) % opts.length] ?? "";
    const extra = H(i, 3) < 0.02;
    const cells: Cell[] = DESIGN_LANES.map((lane, j): Cell => {
      const u = H(i, 10 + j);
      if (lane.kind === "base") return extra ? { k: "a" } : { k: "v", t: base };
      if (extra) return u < 0.6 ? { k: "v", t: H(i, 30 + j) < 0.5 ? base : alt } : { k: "a" };
      const r = lane.rate;
      if (u < r[0]) return { k: "v", t: alt };
      if (u < r[0] + r[1]) return { k: "a" };
      if (u < r[0] + r[1] + r[2]) return kind === "s" ? { k: "e" } : { k: "v", t: alt };
      return { k: "v", t: base };
    });
    const key = (c: Cell): string => (c.k === "a" ? "\u0000a" : c.k === "e" ? "\u0000e" : c.t);
    const counts = new Map<string, number>();
    for (const c of cells) counts.set(key(c), (counts.get(key(c)) ?? 0) + 1);
    let modal = "";
    let most = 0;
    for (const [x, n] of counts) {
      if (n > most) {
        most = n;
        modal = x;
      }
    }
    const differ = cells.some((c) => key(c) !== modal);
    out.push({ i, path: `${prefix}${spec.s}.${group}.${k[0]}`, ch, kind, cells, differ });
  };
  const gen = (quota: number, ch: string | null, specs: Spec[]): void => {
    let count = 0;
    let round = 0;
    const prefix = ch ? `channels.${ch}.` : "core.";
    for (;;) {
      for (const spec of specs) {
        for (const g of spec.g) {
          const group = round ? `${g}-${round + 1}` : g;
          for (const k of spec.k) {
            if (count++ >= quota) return;
            mk(ch, prefix, spec, group, k);
          }
        }
      }
      round += 1;
    }
  };
  gen(440, null, SHSPEC);
  for (const ch of CHANNELS) gen(1520, ch, CHSPEC);
  ROWS = out;
  return out;
}

const TY = { n: "int", s: "string", b: "bool" } as const;
const lineOf = (row: number, lane: number): number => 44 + 8 * row + (SHIFT[lane] ?? 0);

function valueFor(row: DesignRow, cell: Cell): SettingValue | null {
  if (cell.k === "a") return null;
  return { text: cell.k === "e" ? "" : cell.t, ty: cell.k === "e" ? "string" : TY[row.kind], flag: null };
}

function apiCell(row: DesignRow, lane: number): GridCell {
  const cell = row.cells[lane];
  const value = cell === undefined ? null : valueFor(row, cell);
  if (value === null) return { value: null, location: null };
  const line = lineOf(row.i, lane);
  return { value, location: { path: row.path, lines: { start: line, end: line } } };
}

const cursorFor = (offset: number): string => `o${offset}`;
function offsetOf(cursor: string | null): number {
  const match = cursor === null ? null : /^o(\d{1,9})$/.exec(cursor);
  const text = match?.[1];
  return text === undefined ? 0 : Number(text);
}
function limitOf(query: URLSearchParams): number {
  const asked = Number(query.get("limit") ?? DEFAULT_LIMIT);
  return Number.isInteger(asked) && asked >= 1 ? Math.min(asked, GRID_PAGE_MAX) : DEFAULT_LIMIT;
}

/** One page of `GET /api/v1/grid`: the rows of the file for the swimlanes asked for, in the order asked. */
export function designGrid(query: URLSearchParams): Grid {
  const lanes = (query.get("swimlanes") ?? "").split(",").filter((s) => s !== "");
  const columns = lanes.map((id) => DESIGN_LANES.findIndex((lane) => lane.n === id));
  const onlyDiff = query.get("only_differing") === "true";
  const channel = query.get("channel");
  const rows = designRows().filter((row) => (!onlyDiff || row.differ) && (channel === null || row.ch === null || row.ch === channel));
  const from = offsetOf(query.get("cursor"));
  const to = Math.min(rows.length, from + limitOf(query));
  const items: GridRow[] = rows.slice(from, to).map((row) => ({ path: row.path, cells: columns.map((j) => (j < 0 ? { value: null, location: null } : apiCell(row, j))) }));
  return { swimlanes: lanes, rows: { items, next: to < rows.length ? cursorFor(to) : null } };
}

// ---------------------------------------------------------------------------------------------------------------------------
// The comparison of two swimlanes at the core file: one hunk for each setting whose cells differ.

const cellsEqual = (a: Cell | undefined, b: Cell | undefined): boolean => (a === undefined || b === undefined ? a === b : a.k === b.k && (a.k !== "v" || (b.k === "v" && a.t === b.t)));
const emptyLine = (text: string): DiffLine => ({ kind: "context", text, inline_changes: [] });
const siblings = (i: number): { sp: string; sn: string; sn2: string } => ({
  sp: ["min", "enabled", "host", "level"][Math.floor(H(i, 50) * 4)] ?? "min",
  sn: ["timeout", "weight", "retries", "priority"][Math.floor(H(i, 51) * 4)] ?? "timeout",
  sn2: ["tls", "currency", "template", "mask"][Math.floor(H(i, 52) * 4)] ?? "tls",
});

function settingHunk(row: DesignRow, a: Cell, b: Cell, la: number, lb: number): { hunk: DiffHunk; change: SettingChange } {
  const segs = row.path.split(".");
  const leaf = segs[segs.length - 1] ?? row.path;
  const parents = segs.slice(-4, -1);
  const { sp, sn, sn2 } = siblings(row.i);
  const indent = "      ";
  const before: DiffLine[] = [...parents.map((seg, x) => emptyLine(`${" ".repeat(x * 2)}${seg}:`)), emptyLine(`${indent}${sp}: ${sp === "enabled" ? "true" : sp === "host" ? "hsm-sit-01.itm.internal" : sp === "level" ? "INFO" : "1"}`)].slice(-3);
  const after: DiffLine[] = [emptyLine(`${indent}${sn}: ${sn === "timeout" ? "5000" : sn === "weight" ? "50" : sn === "retries" ? "2" : "3"}`), emptyLine(`${indent}${sn2}: ${sn2 === "tls" ? "true" : sn2 === "currency" ? "USD" : sn2 === "template" ? "receipt-v3" : "pan-6-4"}`)];
  const lead = `${indent}${leaf}: `;
  const va = valueFor(row, a);
  const vb = valueFor(row, b);
  const lines: DiffLine[] = [...before];
  if (va !== null) lines.push({ kind: "removed", text: lead + va.text, inline_changes: va.text === "" ? [] : [{ start: lead.length, end: lead.length + va.text.length }] });
  if (vb !== null) lines.push({ kind: "added", text: lead + vb.text, inline_changes: vb.text === "" ? [] : [{ start: lead.length, end: lead.length + vb.text.length }] });
  lines.push(...after);
  const kept = before.length + after.length;
  const hunk: DiffHunk = { old_start: la - before.length, old_lines: kept + (va === null ? 0 : 1), new_start: lb - before.length, new_lines: kept + (vb === null ? 0 : 1), lines };
  const change: SettingChange = {
    path: row.path,
    kind: va === null ? "added" : vb === null ? "removed" : "changed",
    left: va,
    right: vb,
    left_location: va === null ? null : { path: row.path, lines: { start: la, end: la } },
    right_location: vb === null ? null : { path: row.path, lines: { start: lb, end: lb } },
    severity: null,
  };
  return { hunk, change };
}

function laneIndex(ref: string): number {
  const name = /^(?:nfs|effective|baseline):(.+)$/.exec(ref)?.[1];
  return name === undefined ? -1 : DESIGN_LANES.findIndex((lane) => lane.n === name);
}

// ---------------------------------------------------------------------------------------------------------------------------
// The files of a whole-swimlane comparison (the page's `ALL`): a path, how it differs, and for a text file how many lines.

type FileSpec = { file: string; kind: "c" | "b" | "l" | "r"; added: number; removed: number };
const FILES: readonly FileSpec[] = [
  { file: CORE_FILE, kind: "c", added: 7, removed: 5 },
  { file: "tx-infinity-api/tx-infinity-gateway.yml", kind: "c", added: 3, removed: 3 },
  { file: "tx-infinity-api/tx-infinity-ledger.yml", kind: "c", added: 1, removed: 1 },
  { file: "tx-infinity-api/tx-infinity-limits.yml", kind: "l", added: 42, removed: 0 },
  { file: "security/security-roles.yml", kind: "c", added: 4, removed: 1 },
  { file: "security/keystore.jks", kind: "b", added: 0, removed: 0 },
  { file: "resources/receipt-logo.jpeg", kind: "b", added: 0, removed: 0 },
  { file: "resources/receipt-footer.txt", kind: "r", added: 6, removed: 0 },
  { file: "messaging/queues/queue-routes.yml", kind: "c", added: 5, removed: 2 },
  { file: "messaging/queues/dlq-policy.yml", kind: "r", added: 18, removed: 0 },
  { file: "ops/batch/batch-schedule.yml", kind: "c", added: 2, removed: 2 },
  { file: "ops/batch/nightly-jobs.yml", kind: "l", added: 31, removed: 0 },
  { file: "ops/batch/retry-policy.yml", kind: "l", added: 12, removed: 0 },
];
const ON_BRANCH = [CORE_FILE, "tx-infinity-api/tx-infinity-ledger.yml", "messaging/queues/queue-routes.yml", "ops/batch/nightly-jobs.yml"];
const ON_TAG = [CORE_FILE, "tx-infinity-api/tx-infinity-gateway.yml", "security/security-roles.yml", "tx-infinity-api/tx-infinity-limits.yml", "resources/receipt-footer.txt", "messaging/queues/dlq-policy.yml"];

function filesFor(right: string): readonly FileSpec[] {
  if (right.startsWith("git:tenant@")) return FILES.filter((f) => ON_BRANCH.includes(f.file));
  if (right.startsWith("git:base@")) return FILES.filter((f) => ON_TAG.includes(f.file));
  return FILES;
}

const STATE: Record<FileSpec["kind"], TreeCompareEntry["state"]> = { c: "different", b: "different", l: "left_only", r: "right_only" };

/** One page of `GET /api/v1/compare/tree`. `identical` is a comparison with nothing to list. */
export function designTree(query: URLSearchParams, identical = false): TreeComparison {
  const files = identical ? [] : filesFor(query.get("right") ?? "");
  const folders = new Set(files.map((f) => f.file.split("/").slice(0, -1).join("/")));
  const entries: TreeCompareEntry[] = files.map((f) => ({ file: f.file, state: STATE[f.kind] }));
  const from = offsetOf(query.get("cursor"));
  const to = Math.min(entries.length, from + limitOf(query));
  return { entries: { items: entries.slice(from, to), next: to < entries.length ? cursorFor(to) : null }, skipped_identical_subtrees: Math.max(0, 43 - folders.size) };
}

const HEX = "0123456789abcdef";
const hash = (seed: string): string => {
  let out = "";
  for (let i = 0; i < 64; i++) out += HEX[Math.floor(H(i, seed.length * 31 + seed.charCodeAt(i % seed.length)) * 16)] ?? "0";
  return out;
};

const MODS = [["  timeout-ms: 5000", "  timeout-ms: 10000"], ["  retries: 2", "  retries: 3"], ["  enabled: false", "  enabled: true"], ["  max-batch: 200", "  max-batch: 500"]] as const;
const ADDS = ["  jitter-ms: 250", "  dead-letter: true", "  ttl-sec: 3600", "  priority: 2", "  owner: platform-ops", "  notify: ops-sit"];
const DELS = ["  legacy-mode: true", "  poll-ms: 1000"];

function changedFile(spec: FileSpec): DiffHunk[] {
  const m = Math.min(spec.added, spec.removed);
  const ea = spec.added - m;
  const ed = spec.removed - m;
  const lines: DiffLine[] = [emptyLine("settings:")];
  for (let x = 0; x < m; x++) {
    const [old, next] = MODS[x % MODS.length] ?? MODS[0];
    lines.push({ kind: "removed", text: old, inline_changes: [] }, { kind: "added", text: next, inline_changes: [] });
  }
  for (let x = 0; x < ea; x++) lines.push({ kind: "added", text: ADDS[x % ADDS.length] ?? "", inline_changes: [] });
  for (let x = 0; x < ed; x++) lines.push({ kind: "removed", text: DELS[x % DELS.length] ?? "", inline_changes: [] });
  lines.push(emptyLine("  owner: platform-ops"));
  return [{ old_start: 12, old_lines: m + ed + 2, new_start: 12, new_lines: m + ea + 2, lines }];
}

function oneSided(spec: FileSpec, left: boolean): DiffHunk[] {
  const seeds = ["# file", "version: 3", "enabled: true", "entries:", "  - name: primary", "    interval-sec: 300", "    max-attempts: 5", "  - name: secondary", "    interval-sec: 900", "    max-attempts: 3", "timezone: UTC", "owner: platform-ops", "retention-days: 14", "notify:", "  channel: ops-sit"];
  const count = Math.min(15, Math.max(spec.added, spec.removed));
  const lines: DiffLine[] = seeds.slice(0, count).map((text) => ({ kind: left ? "removed" : "added", text, inline_changes: [] }));
  return [left ? { old_start: 1, old_lines: count, new_start: 0, new_lines: 0, lines } : { old_start: 0, old_lines: 0, new_start: 1, new_lines: count, lines }];
}

const base = (query: URLSearchParams, over: Partial<Comparison>): Comparison => ({
  left: query.get("left") ?? "effective:sit",
  right: query.get("right") ?? "effective:sitb",
  file: query.get("file"),
  class: "structured",
  identical: false,
  eol_only: false,
  hunks: [],
  setting_changes: [],
  binary: null,
  ...over,
});

/** `GET /api/v1/compare`: the core file between two swimlanes (the grid's drawer) or any file of the whole-swimlane list. */
export function designCompare(query: URLSearchParams): Comparison {
  const file = query.get("file") ?? CORE_FILE;
  const a = laneIndex(query.get("left") ?? "");
  const b = laneIndex(query.get("right") ?? "");
  const spec = FILES.find((f) => f.file === file);
  // The core file between two of the six swimlanes: one hunk for every setting that differs.
  if (file === CORE_FILE && a >= 0 && b >= 0) {
    const hunks: DiffHunk[] = [];
    const changes: SettingChange[] = [];
    for (const row of designRows()) {
      const ca = row.cells[a];
      const cb = row.cells[b];
      if (ca === undefined || cb === undefined || cellsEqual(ca, cb)) continue;
      const { hunk, change } = settingHunk(row, ca, cb, lineOf(row.i, a), lineOf(row.i, b));
      hunks.push(hunk);
      changes.push(change);
    }
    return base(query, { file, hunks, setting_changes: changes, identical: hunks.length === 0 });
  }
  if (spec === undefined) return base(query, { file, class: "text", identical: true });
  if (spec.kind === "b") {
    const logo = file.includes("logo");
    return base(query, { file, class: "binary", binary: { left: hash(`${file}l`), right: hash(`${file}r`), left_size: logo ? 4300 : 3100, right_size: logo ? 5900 : 3100 } });
  }
  if (spec.kind === "l") return base(query, { file, class: "text", hunks: oneSided(spec, true) });
  if (spec.kind === "r") return base(query, { file, class: "text", hunks: oneSided(spec, false) });
  return base(query, { file, hunks: changedFile(spec) });
}

/** `GET /api/v1/git/refs`: the branches of the tenant repository (one for each tenant) and the tags of the base repository. */
export function designRefs(query: URLSearchParams): { items: Array<{ name: string; kind: "branch" | "tag"; commit: string; branch_kind: "default" | "deployable" | "template" | "common_docs" | "other" | null }>; next: null } {
  const repo = query.get("repo") === "base" ? "base" : "tenant";
  const kind = query.get("kind");
  const prefix = query.get("q") ?? "";
  const all =
    repo === "tenant"
      ? ["sit1", "sit7", "sit9", "sit11", "sit12", "sit13", "templates/common-docs"].map((name) => ({ name, kind: "branch" as const, commit: hash(name).slice(0, 40), branch_kind: name.startsWith("templates/") ? ("common_docs" as const) : ("deployable" as const) }))
      : [
          { name: "main", kind: "branch" as const, commit: hash("main").slice(0, 40), branch_kind: "default" as const },
          ...["v2.3.1", "v2.3.0", "v2.2.4"].map((name) => ({ name, kind: "tag" as const, commit: hash(name).slice(0, 40), branch_kind: null })),
        ];
  return { items: all.filter((ref) => (kind === null || ref.kind === kind) && ref.name.startsWith(prefix)), next: null };
}
