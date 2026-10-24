// The content of the Search page and the Feature Status page of the handoff (`Lanekeeper Search Page.dc.html`,
// `Lanekeeper Feature Status.dc.html`) as contract data: the six swimlanes, the lines the text search finds, the settings the
// settings search finds, the docs, and the flags. The scenarios (./index.ts) and the tests of the three screens both build on
// it, so a test and the design-parity run see the same lane names, counts and lines.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension and `import type`.
import type { DocHit, DocSummary, FlagStatus, RenderedDoc, SettingHit, TextHit } from "../../../api/generated/model/index.ts";

export const NOW = 1_760_000_000_000;
const DAY = 86_400_000;
export const TENANT = "11111111-1111-4111-8111-111111111111";
export const PRIYA = `${TENANT}:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa`;

/** The six swimlanes of the page: tier, id, tenant (the design calls it the branch). */
export const LANES = [
  { tier: "SIT", id: "sit", tenant: "sit1" },
  { tier: "SIT", id: "sitb", tenant: "sit7" },
  { tier: "SIT", id: "sitc", tenant: "sit9" },
  { tier: "PRESIT", id: "presita", tenant: "sit11" },
  { tier: "PRESIT", id: "presitb", tenant: "sit12" },
  { tier: "PRESIT", id: "presitc", tenant: "sit13" },
] as const;

const DIR = "tx-infinity-api";
const BASE_FILE = `${DIR}/tx-infinity-core.yml`;
const GATEWAY_FILE = `${DIR}/tx-infinity-gateway.yml`;
const tenantFile = (tenant: string) => `${DIR}/tx-infinity-core-${tenant}.yml`;

/** The lines of the base file that mention the settings of the page, and the tenant overrides of each lane. */
const BASE_LINES: ReadonlyArray<readonly [number, string]> = [
  [8, "remote-itm-teller:"],
  [12, "timeoutMs: 30000"],
  [13, "enableAccountSorting: false  # sorts by display name when true"],
  [17, "backoffMs: 250"],
  [21, "atm-iso:"],
  [24, "timeoutMs: 45000"],
  [27, "enableAccountSorting: false"],
  [34, "level: INFO"],
  [49, "limitCents: 500000"],
];
const TENANT_LINES: Record<string, ReadonlyArray<readonly [number, string]>> = {
  sit: [[12, "timeoutMs: 60000"], [13, "enableAccountSorting: true"]],
  sitb: [[13, "enableAccountSorting: true"]],
  sitc: [[12, "timeoutMs: 60000"], [13, "enableAccountSorting: true"], [27, "enableAccountSorting: true"]],
  presita: [],
  presitb: [[13, "enableAccountSorting: true"]],
  presitc: [],
};
const GATEWAY_LINES: ReadonlyArray<readonly [number, string]> = [
  [18, "timeoutMs: 15000"],
  [41, "ACCOUNT_SORTING: ${enableAccountSorting}"],
  [57, "upstreamTimeoutMs: 20000"],
];

/** What a text search asks for: the same switches as the screen (literal unless `regex`, ignoring case unless `caseSensitive`). */
export type TextQuery = { pattern: string; regex?: boolean; caseSensitive?: boolean; lanes?: readonly string[]; glob?: string };

/** The byte offsets of the first match of `pattern` in `text`, as the server reports them, or undefined. Throws on a bad regex. */
function matchRange(text: string, pattern: string, regex: boolean, caseSensitive: boolean): [number, number] | undefined {
  let start = -1;
  let length = 0;
  if (regex) {
    const found = new RegExp(pattern, caseSensitive ? "" : "i").exec(text);
    if (found && found[0] !== "") {
      start = found.index;
      length = found[0].length;
    }
  } else {
    start = (caseSensitive ? text : text.toLowerCase()).indexOf(caseSensitive ? pattern : pattern.toLowerCase());
    length = pattern.length;
  }
  if (start < 0) return undefined;
  const bytes = (part: string) => new TextEncoder().encode(part).length;
  return [bytes(text.slice(0, start)), bytes(text.slice(0, start + length))];
}

/** A path glob as `**` and `*` mean it: `**` crosses folders, `*` does not. */
function globMatches(glob: string, path: string): boolean {
  let source = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i] ?? "";
    if (c === "*") {
      if (glob[i + 1] === "*") {
        source += ".*";
        i += 1;
        if (glob[i + 1] === "/") i += 1;
      } else source += "[^/]*";
    } else if (c === "?") source += "[^/]";
    else source += c.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${source}$`).test(path);
}

function hitAt(swimlane: string, path: string, line: number, text: string, range: [number, number], tenant: string | null): TextHit {
  return { swimlane, path, line, text, ranges: [range], logical_file: path.includes("-sit") ? BASE_FILE : path, ...(tenant === null ? {} : { tenant }) };
}

/**
 * What a text search finds on the six swimlanes of the page, in swimlane order, tenant file before base file. A bad regex throws,
 * as the server answers it with a 400.
 */
export function textHitsFor(query: TextQuery | string): TextHit[] {
  const q: TextQuery = typeof query === "string" ? { pattern: query } : query;
  const lanes = q.lanes ?? LANES.map((l) => l.id);
  const hits: TextHit[] = [];
  const visit = (swimlane: string, path: string, rows: ReadonlyArray<readonly [number, string]>, tenant: string | null) => {
    if (q.glob !== undefined && q.glob !== "" && !globMatches(q.glob, path)) return;
    for (const [line, text] of rows) {
      const range = matchRange(text, q.pattern, q.regex === true, q.caseSensitive === true);
      if (range) hits.push(hitAt(swimlane, path, line, text, range, tenant));
    }
  };
  for (const { id, tenant } of LANES) {
    if (!lanes.includes(id)) continue;
    visit(id, tenantFile(tenant), TENANT_LINES[id] ?? [], tenant);
    visit(id, BASE_FILE, BASE_LINES, null);
    visit(id, GATEWAY_FILE, GATEWAY_LINES, null);
  }
  return hits;
}

/** Settings whose path holds `needle`: the base file's values, and a tenant file's override beside them. */
export function settingHitsFor(needle: string, lanes: readonly string[] = LANES.map((l) => l.id)): SettingHit[] {
  const hits: SettingHit[] = [];
  const hit = (swimlane: string, file: string, path: string, value: string, line: number, tenant: string | null): SettingHit => ({
    swimlane,
    file,
    path,
    value: { text: value, ty: value === "true" || value === "false" ? "bool" : /^\d+$/.test(value) ? "int" : "string", flag: null },
    location: { path, lines: { start: line, end: line } },
    logical_file: BASE_FILE,
    ...(tenant === null ? {} : { tenant }),
  });
  for (const { id, tenant } of LANES) {
    if (!lanes.includes(id)) continue;
    for (const [line, text] of TENANT_LINES[id] ?? []) {
      const key = text.split(":")[0] ?? "";
      const path = line >= 27 ? `atm-iso.${key}` : `remote-itm-teller.${key}`;
      if (path.toLowerCase().includes(needle.toLowerCase())) hits.push(hit(id, tenantFile(tenant), path, text.split(": ")[1] ?? "", line, tenant));
    }
    for (const [line, text] of BASE_LINES) {
      if (!text.includes(": ")) continue;
      const key = text.split(":")[0] ?? "";
      const path = line >= 21 ? `atm-iso.${key}` : `remote-itm-teller.${key}`;
      if (path.toLowerCase().includes(needle.toLowerCase())) hits.push(hit(id, BASE_FILE, path, (text.split(": ")[1] ?? "").split("  #")[0] ?? "", line, null));
    }
  }
  return hits;
}

/** `count` hits of `needle` spread over the six swimlanes and forty files, for a search that reaches the server's cap. */
export function manyTextHits(needle: string, count: number): TextHit[] {
  return Array.from({ length: count }, (_, i) => {
    const text = `${needle}: ${i}`;
    const swimlane = LANES[Math.floor(i / 84) % LANES.length]?.id ?? "sit";
    return hitAt(swimlane, `${DIR}/service-${String(i % 40).padStart(2, "0")}.yml`, (i % 90) + 1, text, [0, new TextEncoder().encode(needle).length], null);
  });
}

/** The two docs of the page: one synced from Git (read-only), one uploaded. */
export const GIT_DOC: DocSummary = { id: 101, title: "Account sorting", path: "/git/main/docs/account-sorting.md", source: "git", version: 3, size: 4096, updated_at: NOW - 21 * DAY, updated_by: null };
export const UPLOAD_DOC: DocSummary = { id: 102, title: "Instant payments", path: "/uploads/instant-payments.md", source: "upload", version: 3, size: 18 * 1024, updated_at: NOW - 2 * DAY, updated_by: PRIYA };
export const RECEIPTS_DOC: DocSummary = { id: 103, title: "Receipts v3", path: "/git/main/docs/receipts-v3.md", source: "git", version: 3, size: 3072, updated_at: NOW - 21 * DAY, updated_by: null };
export const DOCS: readonly DocSummary[] = [GIT_DOC, UPLOAD_DOC, RECEIPTS_DOC];

const hash = (seed: string): string => seed.repeat(Math.ceil(64 / seed.length)).slice(0, 64);
const HASH_A = hash("a3f9c21");
const HASH_B = hash("7d20e8b");

/** The page the server renders for a doc: sanitised HTML with prefixed heading ids, as the allowlist of `renderDoc` states. */
export function renderedDoc(doc: DocSummary): RenderedDoc {
  if (doc.id === UPLOAD_DOC.id) {
    return {
      path: doc.path,
      content_hash: HASH_B,
      headings: [
        { id: "doc-overview", level: 2, text: "Overview", line: 3 },
        { id: "doc-feature-flags", level: 2, text: "Feature Flags", line: 7 },
        { id: "doc-limits", level: 2, text: "Limits", line: 15 },
        { id: "doc-rollback", level: 2, text: "Rollback", line: 19 },
      ],
      html:
        '<h2 id="doc-overview">Overview</h2><p>Instant payments settle through the instant rail instead of the nightly batch. The rail is off until <code>enableInstantPayments</code> is set.</p>' +
        '<h2 id="doc-feature-flags">Feature Flags</h2><table><thead><tr><th>Flag</th><th>Channel</th><th>Template default</th><th>Effect</th></tr></thead><tbody><tr><td><code>enableInstantPayments</code></td><td>all channels</td><td><code>false</code></td><td>Turns on the instant rail.</td></tr></tbody></table><pre><code>featureFlags:\n  instantPayments: true\n</code></pre>' +
        '<h2 id="doc-limits">Limits</h2><p>Limits are set per entry group in <code>atm-iso</code>.</p>' +
        '<h2 id="doc-rollback">Rollback</h2><ul><li>Set <code>enableInstantPayments</code> back to <code>false</code>.</li><li>Restart <code>tx-infinity-core</code> on the swimlane.</li></ul>',
    };
  }
  return {
    path: doc.path,
    content_hash: HASH_A,
    headings: [
      { id: "doc-overview", level: 2, text: "Overview", line: 3 },
      { id: "doc-feature-flags", level: 2, text: "Feature Flags", line: 7 },
      { id: "doc-rollout", level: 2, text: "Rollout", line: 14 },
      { id: "doc-troubleshooting", level: 2, text: "Troubleshooting", line: 19 },
    ],
    html:
      '<h2 id="doc-overview">Overview</h2><p>Account sorting orders the account list returned to <code>remote-itm-teller</code> and <code>atm-iso</code> by display name. It changes ordering only, never balances, limits or entitlements.</p>' +
      '<h2 id="doc-feature-flags">Feature Flags</h2><p>Each channel reads its own flag. When the tenant file and the base file both set a value, the tenant file wins.</p><table><thead><tr><th>Flag</th><th>Channel</th><th>Template default</th><th>Effect</th></tr></thead><tbody><tr><td><code>enableAccountSorting</code></td><td><code>remote-itm-teller</code></td><td><code>false</code></td><td>Sorts the account picker by display name.</td></tr><tr><td><code>enableAccountSorting</code></td><td><code>atm-iso</code></td><td><code>false</code></td><td>Sorts accounts on the ATM account screen.</td></tr></tbody></table><pre><code>remote-itm-teller:\n  enableAccountSorting: true\n  sortOrder: asc\n</code></pre>' +
      '<h2 id="doc-rollout">Rollout</h2><ul><li>Enable on <code>sit</code> first and compare it with <code>sitb</code> before widening.</li><li>Check Findings for ordering mismatches for one working day.</li></ul>' +
      '<h2 id="doc-troubleshooting">Troubleshooting</h2><p>If account sorting looks wrong after a change, check that the tenant file and the base file agree for the channel, then restart <code>tx-infinity-core</code>.</p>',
  };
}

/** The sections the docs search finds (title, heading path, snippet, anchor), as the page's data lists them. */
export const DOC_SECTIONS: ReadonlyArray<{ doc: DocSummary; headings: string[]; anchor: string | null; snippet: string }> = [
  { doc: GIT_DOC, headings: ["Account sorting", "Overview"], anchor: "doc-overview", snippet: "Account sorting orders the account list returned to `remote-itm-teller` and `atm-iso` by display name. It changes ordering only, never balances or limits." },
  { doc: GIT_DOC, headings: ["Account sorting", "Feature Flags"], anchor: "doc-feature-flags", snippet: "`enableAccountSorting` turns on account sorting for a channel. The template default is `false`. Set it per channel in the tenant file; the tenant value wins over the base file." },
  { doc: GIT_DOC, headings: ["Account sorting", "Troubleshooting"], anchor: "doc-troubleshooting", snippet: "If account sorting looks wrong after a change, check that the tenant file and the base file agree for the channel, then restart `tx-infinity-core`." },
  { doc: RECEIPTS_DOC, headings: ["Receipts v3", "Display order"], anchor: "doc-display-order", snippet: "Receipt lines follow the account sorting set by `enableAccountSorting` for the same channel." },
  { doc: UPLOAD_DOC, headings: ["Instant payments", "Feature Flags"], anchor: "doc-feature-flags", snippet: "`enableInstantPayments` opens the instant rail for every channel. The template default is `false`. Pair it with the limits below before enabling." },
  { doc: UPLOAD_DOC, headings: ["Instant payments", "Limits"], anchor: "doc-limits", snippet: "Limits are set per entry group in `atm-iso`. Account sorting does not affect them." },
];

/** What a docs search for the words of `query` finds: every word must appear in the title, the heading path or the snippet. */
export function docHitsFor(query: string): DocHit[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  return DOC_SECTIONS.filter((s) => {
    const hay = `${s.headings.join(" ")} ${s.snippet}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  }).map((s, i) => ({
    doc: s.doc.id,
    title: s.doc.title,
    path: s.doc.path,
    heading_path: s.headings,
    snippet: s.snippet,
    score: 1 - i * 0.05,
    related_files: [BASE_FILE],
    ...(s.anchor === null ? {} : { anchor: s.anchor }),
  }));
}

const flag = (name: string, setting: string, def: string, channel: string | null, values: Record<string, string>, anchor: string): FlagStatus => ({
  flag: name,
  file: BASE_FILE,
  channel,
  setting,
  template_default: { text: def, ty: "bool", flag: null },
  values: LANES.map(({ id }) => ({ swimlane: id, value: { text: values[id] ?? def, ty: "bool", flag: null } })),
  anchor,
});

/** The documented flags of the Feature Status page: a flag with two channels is two rows, with the channel named. */
export const FLAGS: readonly FlagStatus[] = [
  flag("enableAccountSorting", "remote-itm-teller.enableAccountSorting", "false", "remote-itm-teller", { sit: "true", sitb: "true", sitc: "true", presitb: "true" }, "doc-feature-flags"),
  flag("enableAccountSorting", "atm-iso.enableAccountSorting", "false", "atm-iso", { sitc: "true" }, "doc-feature-flags"),
  flag("enableInstantPayments", "featureFlags.instantPayments", "false", null, { sitb: "true" }, "doc-feature-flags"),
  flag("enableHoldsV2", "featureFlags.holdsV2", "true", null, { presita: "false" }, "doc-feature-flags"),
  flag("enableReceiptsV3", "remote-itm-teller.enableReceiptsV3", "true", "remote-itm-teller", {}, "doc-feature-flags"),
  flag("enableReceiptsV3", "atm-iso.enableReceiptsV3", "true", "atm-iso", { presitc: "false" }, "doc-feature-flags"),
  flag("enableLegacyBatch", "featureFlags.legacyBatch", "false", null, { presita: "true" }, "doc-legacy"),
  flag("enableAuditTrail", "featureFlags.auditTrail", "true", null, {}, "doc-feature-flags"),
  flag("enableRateLimiter", "remote-itm-teller.enableRateLimiter", "true", "remote-itm-teller", {}, "doc-feature-flags"),
  flag("enableRateLimiter", "atm-iso.enableRateLimiter", "true", "atm-iso", { sitb: "false" }, "doc-feature-flags"),
  flag("enableCashRecycling", "atm-iso.enableCashRecycling", "false", "atm-iso", { presita: "true" }, "doc-feature-flags"),
];
