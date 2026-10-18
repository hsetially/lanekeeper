// The data of the file view's design scenarios (S3): the files of the File page of the handoff (`Lanekeeper File.dc.html`) and the
// things around them (versions, a comparison, docs, flags, pull-request previews). Synthetic, made up for the design; nothing here
// is a real tenant's configuration. Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension
// and `import type`.
import type { Comparison, DocHit, FileContent, FileEntry, FlagStatus, PrJob, PrLink, PrPreview, Version } from "../../../api/generated/model/index.ts";
import { MARCO, PRIYA } from "../shell/index.ts";

export { MARCO, PRIYA };

export const LANE = "sitb";
export const TENANT = "sit7";
export const NOW = 1_760_000_000_000;

/** A 64-character hash from a short seed, so every file and version in the scenarios has a stable one. */
export const hash = (seed: string): string => seed.repeat(Math.ceil(64 / seed.length)).slice(0, 64);
export const COMMIT = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";

export const CORE_PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
export const CORE_LOGICAL = "tx-infinity-api/tx-infinity-core.yml";
export const H_NFS = hash("a3f9c21");
export const H_GIT = hash("5be1c04");
export const H_BASELINE = hash("9d3a7f2");
export const H_NEWER = hash("c19a4e2");

const MASKED_SECRET = "[REDACTED: 22 chars, generic_secret]";

/** The lines of the core file, up to `total` lines, as the design draws it (a header, the options, kafka, logging, then more of the same). */
export function coreLines(total: number, options: { secret: "masked" | "env" | "none" } = { secret: "env" }): string[] {
  const head = [
    "# tx-infinity-core · tenant override for sit7",
    "tenant: sit7",
    "service:",
    "  name: tx-infinity-core",
    "  replicas: 2",
    "  resources:",
    "    requests:",
    "      cpu: 500m",
    "      memory: 1Gi",
    "txInfinityOptions:",
    "  remote-itm-teller:",
    "    timeoutMs: 30000",
    "    enableAccountSorting: true",
    "    sortOrder: asc",
    "    pageSize: 50",
    "    retry:",
    "      attempts: 3",
    "      backoffMs: 250",
    options.secret === "masked" ? `    clientSecret: ${MASKED_SECRET}` : options.secret === "env" ? "    clientSecret: ${TELLER_CLIENT_SECRET}" : "    clientId: teller",
    "  remote-itm-vault:",
    "    timeoutMs: 30000",
    "    enableAccountSorting: false",
    "    pageSize: 25",
    "kafka:",
    "  bootstrap: kafka-sit7:9092",
    "  consumerGroup: tx-core-sit7",
    "  autoOffsetReset: latest",
    "logging:",
    "  appender: json",
    "  level: INFO",
    "  maskPatterns:",
    "    - pan",
    "    - accountNumber",
  ];
  const lines = head.slice(0, total);
  for (let n = 1; lines.length < total; n++) {
    lines.push(`zone-${n}:`, `  enabled: true`, `  weight: ${n}`);
  }
  return lines.slice(0, total);
}

/** The text with the line ending of the file. */
export const joinLines = (lines: readonly string[], eol: "\r\n" | "\n"): string => `${lines.join(eol)}${eol}`;

/** The 0-based UTF-8 byte offset of the start of a 1-based line, in a text whose lines end with `eol`. All the text is ASCII here. */
export function lineStart(lines: readonly string[], line: number, eol: "\r\n" | "\n"): number {
  let at = 0;
  for (let i = 0; i < line - 1; i++) at += (lines[i]?.length ?? 0) + eol.length;
  return at;
}

export type FileKit = {
  path: string;
  logical: string | null;
  tenant: string | null;
  content: FileContent;
  entry: FileEntry;
};

const attribution = (user: string, source: "tool_write" | "nfs_client" = "tool_write") => ({ source, confidence: "certain" as const, actor: { kind: "user" as const, user }, evidence: {} });

function entryOf(path: string, over: Partial<FileEntry>): FileEntry {
  return {
    path,
    is_dir: false,
    kind: "tenant",
    class: "structured",
    role: "property_source",
    size: 0,
    hash: null,
    drift: "nfs_ahead",
    format: { eol: "crlf", encoding: "utf8", bom: false },
    content_withheld: false,
    attribution: attribution(PRIYA),
    severity: "medium",
    pickup_state: "needs_notify_or_restart",
    logical_file: null,
    tenant: null,
    ...over,
  };
}

/** The tenant file of the core service: 412 lines, CRLF, one masked secret (so it opens read-only) or none (so it can be edited). */
export function coreFile(options: { masked: boolean }): FileKit {
  const secret = options.masked ? "masked" : "env";
  const lines = coreLines(412, { secret });
  const text = joinLines(lines, "\r\n");
  const masks = options.masked ? [{ line: 19, start: lineStart(lines, 19, "\r\n") + "    clientSecret: ".length, end: lineStart(lines, 19, "\r\n") + "    clientSecret: ".length + MASKED_SECRET.length, setting: "txInfinityOptions.remote-itm-teller.clientSecret" }] : undefined;
  const content: FileContent = { path: CORE_PATH, hash: H_NFS, class: "structured", format: { eol: "crlf", encoding: "utf8", bom: false }, size: text.length, content_withheld: false, text, ...(masks === undefined ? {} : { masks }) };
  return { path: CORE_PATH, logical: CORE_LOGICAL, tenant: TENANT, content, entry: entryOf(CORE_PATH, { size: text.length, hash: H_NFS, logical_file: CORE_LOGICAL, tenant: TENANT }) };
}

/** A file whose line endings differ from Git's only (the design's "Line endings only" note). */
export function lineEndingFile(): FileKit {
  const path = "tx-infinity-api/tx-infinity-gateway.yml";
  const text = joinLines(coreLines(214, { secret: "none" }), "\r\n");
  const content: FileContent = { path, hash: hash("d41e07b"), class: "structured", format: { eol: "crlf", encoding: "utf8", bom: false }, size: text.length, content_withheld: false, text };
  return { path, logical: path, tenant: null, content, entry: entryOf(path, { kind: "base", size: text.length, hash: content.hash, logical_file: path }) };
}

/** A file of 6,140 lines: it opens read-only first, and the editor is enabled on request. */
export function largeFile(): FileKit {
  const path = "security/security-roles.yml";
  const text = joinLines(coreLines(6140, { secret: "none" }), "\r\n");
  const content: FileContent = { path, hash: hash("0be9c5a"), class: "structured", format: { eol: "crlf", encoding: "utf8", bom: false }, size: text.length, content_withheld: false, text };
  return { path, logical: path, tenant: null, content, entry: entryOf(path, { kind: "base", drift: "in_sync", size: text.length, hash: content.hash, logical_file: path, severity: null, pickup_state: null }) };
}

/** A picture: no text, shown from the blob store. */
export function binaryFile(): FileKit {
  const path = "document-service/resources/receipt-logo.jpeg";
  const content: FileContent = { path, hash: hash("b71c4e9"), class: "binary", format: { eol: "lf", encoding: "utf8", bom: false }, size: 18_432, content_withheld: false, text: null };
  return { path, logical: path, tenant: null, content, entry: entryOf(path, { kind: "base", class: "binary", role: "resource", size: 18_432, hash: content.hash, logical_file: path, format: null, severity: null, pickup_state: null }) };
}

/** A file whose content is withheld by policy: there is no text and no blob. */
export function deniedFile(): FileKit {
  const path = "security/keystore.jks";
  const content: FileContent = { path, hash: hash("9d2c47e"), class: "denied", format: { eol: "lf", encoding: "utf8", bom: false }, size: 3277, content_withheld: true, text: null };
  return { path, logical: null, tenant: null, content, entry: entryOf(path, { kind: "untracked", class: "denied", role: "resource", size: 3277, hash: content.hash, drift: "untracked", format: null, content_withheld: true, severity: null, pickup_state: null, attribution: null }) };
}

/** The same core file as Git has it: before the two changes the design's Diff tab shows. LF endings. */
export function coreGitText(): string {
  const lines = coreLines(412, { secret: "env" }).map((line) => (line === "    enableAccountSorting: true" ? "    enableAccountSorting: false" : line)).filter((line) => line !== "    sortOrder: asc");
  return joinLines(lines.map((line) => (line === "  level: INFO" ? "  level: WARN" : line)), "\n");
}

export function coreBaselineText(): string {
  return joinLines(coreLines(410, { secret: "env" }), "\r\n");
}

/**
 * The comparison of Git's core file with NFS's, as the engine would compute it (the design's three changes). It is true of the texts
 * `coreGitText()` and `coreFile().content.text` (consistency.test.ts): Git's file has no `sortOrder` line, so everything after it is
 * one line higher there than on NFS. The design's own hunk headers (`-10,9 +10,10`, `-28,6 +29,6`) are a little looser than that.
 */
export function coreComparison(left: string, right: string): Comparison {
  const value = (text: string, ty: "bool" | "string") => ({ text, ty, flag: null });
  const at = (line: number) => ({ path: CORE_PATH, lines: { start: line, end: line } });
  return {
    left,
    right,
    file: CORE_LOGICAL,
    class: "structured",
    identical: false,
    eol_only: false,
    hunks: [
      {
        old_start: 10,
        old_lines: 8,
        new_start: 10,
        new_lines: 9,
        lines: [
          { kind: "context", text: "txInfinityOptions:", inline_changes: [] },
          { kind: "context", text: "  remote-itm-teller:", inline_changes: [] },
          { kind: "context", text: "    timeoutMs: 30000", inline_changes: [] },
          { kind: "removed", text: "    enableAccountSorting: false", inline_changes: [{ start: 26, end: 31 }] },
          { kind: "added", text: "    enableAccountSorting: true", inline_changes: [{ start: 26, end: 30 }] },
          { kind: "added", text: "    sortOrder: asc", inline_changes: [] },
          { kind: "context", text: "    pageSize: 50", inline_changes: [] },
          { kind: "context", text: "    retry:", inline_changes: [] },
          { kind: "context", text: "      attempts: 3", inline_changes: [] },
          { kind: "context", text: "      backoffMs: 250", inline_changes: [] },
        ],
      },
      {
        old_start: 27,
        old_lines: 6,
        new_start: 28,
        new_lines: 6,
        lines: [
          { kind: "context", text: "logging:", inline_changes: [] },
          { kind: "context", text: "  appender: json", inline_changes: [] },
          { kind: "removed", text: "  level: WARN", inline_changes: [{ start: 9, end: 13 }] },
          { kind: "added", text: "  level: INFO", inline_changes: [{ start: 9, end: 13 }] },
          { kind: "context", text: "  maskPatterns:", inline_changes: [] },
          { kind: "context", text: "    - pan", inline_changes: [] },
          { kind: "context", text: "    - accountNumber", inline_changes: [] },
        ],
      },
    ],
    setting_changes: [
      { path: "txInfinityOptions.remote-itm-teller.enableAccountSorting", kind: "changed", left: value("false", "bool"), right: value("true", "bool"), left_location: at(13), right_location: at(13), severity: null },
      { path: "txInfinityOptions.remote-itm-teller.sortOrder", kind: "added", left: null, right: value("asc", "string"), left_location: null, right_location: at(14), severity: null },
      { path: "logging.level", kind: "changed", left: value("WARN", "string"), right: value("INFO", "string"), left_location: at(29), right_location: at(30), severity: null },
    ],
    binary: null,
  };
}

export function lineEndingComparison(left: string, right: string): Comparison {
  return { left, right, file: "tx-infinity-api/tx-infinity-gateway.yml", class: "structured", identical: false, eol_only: true, hunks: [], setting_changes: [], binary: null };
}

export function binaryComparison(left: string, right: string): Comparison {
  return { left, right, file: "document-service/resources/receipt-logo.jpeg", class: "binary", identical: false, eol_only: false, hunks: [], setting_changes: [], binary: { left: hash("b11c4e0"), right: hash("b71c4e9"), left_size: 16_384, right_size: 18_432 } };
}

/** The versions of the core file the History panel shows, newest first: the design's four rows. */
export function coreHistory(): Version[] {
  return [
    { hash: H_NFS, observed_at: NOW - 5 * 60_000, source: "tool_write", size: 9_100, attribution: attribution(PRIYA), severity: "medium", commit: null },
    { hash: H_NEWER, observed_at: NOW - 2 * 3_600_000, source: "tool_write", size: 9_050, attribution: attribution(MARCO), severity: "low", commit: null },
    { hash: hash("7e22b01"), observed_at: NOW - 26 * 3_600_000, source: "scan", size: 9_040, attribution: { source: "nfs_client", confidence: "medium", actor: null, evidence: { executable: "vim" } }, severity: null, commit: null },
    { hash: H_GIT, observed_at: NOW - 5 * 86_400_000, source: "sync", size: 8_980, attribution: { source: "sync_job", confidence: "high", actor: null, evidence: {} }, severity: null, commit: COMMIT },
  ];
}

export function coreDocs(): DocHit[] {
  return [
    {
      doc: 12,
      title: "Account sorting",
      path: "/git/main/docs/remote-itm-teller.md",
      heading_path: ["remote-itm-teller", "Account sorting"],
      snippet: "Lets tellers sort an account list by balance or name. Turned on per swimlane with a feature flag in this file.",
      score: 0.92,
      related_files: [CORE_LOGICAL],
      anchor: "doc-account-sorting",
    },
  ];
}

export function coreFlags(): FlagStatus[] {
  const on = { text: "true", ty: "bool" as const, flag: null };
  const off = { text: "false", ty: "bool" as const, flag: null };
  const flag = (name: string, setting: string, value: typeof on, fallback: typeof on): FlagStatus => ({ flag: name, file: CORE_LOGICAL, channel: null, setting, template_default: fallback, values: [{ swimlane: LANE, value }], anchor: null });
  return [
    flag("enableAccountSorting", "txInfinityOptions.remote-itm-teller.enableAccountSorting", on, off),
    flag("enableAccountSorting", "txInfinityOptions.remote-itm-vault.enableAccountSorting", off, off),
  ];
}

const prFile = (repo: string, existing: boolean) => ({ nfs_path: CORE_PATH, repo_path: repo, exists_on_branch: existing });

export function prPreview(kind: "tenant" | "base", over: Partial<PrPreview> = {}): PrPreview {
  return kind === "tenant"
    ? { repo: "csp/csp-tenant-data", destination: { kind: "tenant", branch: TENANT }, base_branch: TENANT, files: [prFile("data/config/tx-infinity-api/tx-infinity-core.yml", true)], unchanged: [], warnings: [], open_pr: null, ...over }
    : { repo: "csp/configuration-base-saas", destination: { kind: "base" }, base_branch: "main", files: [prFile("config/tx-infinity-api/tx-infinity-core.yml", true)], unchanged: [], warnings: ["A tenant file is copied into base. Every swimlane gets it on its next sync."], open_pr: null, ...over };
}

export const openPr: PrLink = { id: 3, swimlane: LANE, path: CORE_PATH, nfs_hash: H_NFS, state: "open", pr_url: "https://github.com/csp/csp-tenant-data/pull/418", pr_number: 418, author: PRIYA, created_at: NOW - 3_600_000, closed_at: null };

export function prJob(state: PrJob["state"], over: Partial<PrJob> = {}): PrJob {
  return { id: 7, swimlane: LANE, state, paths: [CORE_PATH], pr_url: state === "completed" ? "https://github.com/csp/csp-tenant-data/pull/419" : null, error: null, created_at: NOW, updated_at: NOW + 4_000, ...over };
}
