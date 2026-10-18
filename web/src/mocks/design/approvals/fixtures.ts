// The data of the Approvals page of the design (`Lanekeeper Approvals Page.dc.html`) as the contract's types: the people, the twelve
// proposals the page draws (AP-2048 is the one that arrives live; the other eleven are in the four tabs), and a builder for the
// tests. The times are derived from the frozen clock the visual runs use, so "12 min", "5 h" and "1 d" come out as the page says.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`, nothing else.
import type { Attribution, AuditEvent, DiffHunk, DiffLine, Proposal, SettingChange, User } from "../../../api/generated/model/index.ts";

export const NOW = 1_760_000_000_000;
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

const TENANT = "11111111-1111-4111-8111-111111111111";
const uid = (digit: string): string => `${TENANT}:${digit.repeat(8)}-${digit.repeat(4)}-4${digit.repeat(3)}-8${digit.repeat(3)}-${digit.repeat(12)}`;

// Priya Shah is the signed-in Admin (the same id as the Shell scenario's). Amir and Lena are the other two Admins the page names.
export const PRIYA = uid("a");
export const AMIR = uid("c");
export const LENA = uid("d");
export const DANA = uid("e");
export const ANA = uid("f");
export const WEN = uid("1");
export const TOMAS = uid("2");

const person = (id: string, name: string, role: User["role"]): User => ({
  id,
  email: `${name.toLowerCase().replace(/[^a-z]+/g, ".")}@example.test`,
  display_name: name,
  role,
  status: "active",
  requires_approval: false,
  github_login: null,
  last_seen_at: NOW - HOUR,
});

export const PEOPLE: readonly User[] = [
  person(PRIYA, "Priya Shah", "admin"),
  person(AMIR, "Amir Haddad", "admin"),
  person(LENA, "Lena Brandt", "admin"),
  person(DANA, "Dana Okoye", "editor"),
  person(ANA, "Ana Ruiz", "editor"),
  person(WEN, "Wen Zhao", "editor"),
  person(TOMAS, "Tomás Reyes", "editor"),
];

/** A 64-hex hash that starts with the eight characters the page shows ("a91f3c0e"). */
export const hash = (head: string): string => (head + "0".repeat(64)).slice(0, 64);
const ZERO = "0".repeat(64);

type Row = readonly [kind: DiffLine["kind"], text: string];
const ctx = (text: string): Row => ["context", text];
const del = (text: string): Row => ["removed", text];
const add = (text: string): Row => ["added", text];

/** One hunk from its lines: the counts follow from the kinds. */
export function hunk(oldStart: number, newStart: number, rows: readonly Row[]): DiffHunk {
  const old = rows.filter(([kind]) => kind !== "added").length;
  const now = rows.filter(([kind]) => kind !== "removed").length;
  return { old_start: oldStart, old_lines: old, new_start: newStart, new_lines: now, lines: rows.map(([kind, text]) => ({ kind, text, inline_changes: [] })) };
}

const value = (text: string, ty: "string" | "int" | "bool" = "string") => ({ text, ty, flag: null }) as const;
const change = (path: string, left: string, right: string, line: number, severity: SettingChange["severity"] = null): SettingChange => ({
  path,
  kind: "changed",
  left: value(left, "int"),
  right: value(right, "int"),
  left_location: { path, lines: { start: line, end: line } },
  right_location: { path, lines: { start: line, end: line } },
  severity,
});

export type Make = Partial<Proposal> & Pick<Proposal, "id" | "author" | "swimlane" | "path">;

/** A proposal with the fields the contract requires; everything else is what the test or the page says. */
export function proposal(make: Make): Proposal {
  return {
    expected: { state: "hash", hash: hash("a91f3c0e") },
    new_hash: hash("7c3e55d1"),
    status: "pending",
    hunks: [],
    setting_changes: [],
    severity: null,
    created_at: NOW - 12 * MIN,
    expires_at: NOW + 24 * HOUR - 12 * MIN,
    decided_by: null,
    decision_comment: null,
    action: "edit",
    via: "ui",
    ...make,
  };
}

const T = "tx-infinity-api";

/** AP-2048: the proposal that arrives while the page is open (frame 1s). Cursor-made, by Dana, who can not approve it. */
export const ARRIVAL: Proposal = proposal({
  id: 2048,
  author: DANA,
  via: "mcp",
  swimlane: "presita",
  path: "kafka/kafka-topics-sit11.yml",
  created_at: NOW - 3_000,
  expires_at: NOW + 24 * HOUR - 3_000,
  expected: { state: "hash", hash: hash("5b20e1aa") },
  new_hash: hash("3c9f7d02"),
  hunks: [hunk(40, 40, [ctx("  payments.retry:"), del("    partitions: 3"), add("    partitions: 6"), ctx("    replication: 3"), ctx("    retentionHours: 72")])],
});

export const PENDING: readonly Proposal[] = [
  proposal({
    id: 2047,
    author: WEN,
    via: "mcp",
    swimlane: "sitc",
    path: `${T}/tx-infinity-payments-sit9.yml`,
    created_at: NOW - 4 * MIN,
    expires_at: NOW + 24 * HOUR - 4 * MIN,
    expected: { state: "hash", hash: hash("d84c07f3") },
    new_hash: hash("19ab6e40"),
    hunks: [hunk(55, 55, [ctx("callbacks:"), del("  timeoutMs: 8000"), add("  timeoutMs: 12000"), ctx("  url: https://payments-sit9.corp.internal/hooks"), ctx("  retries: 3")])],
    setting_changes: [change("callbacks.timeoutMs", "8000", "12000", 56)],
  }),
  proposal({
    id: 2046,
    author: ANA,
    swimlane: "sitb",
    path: `${T}/tx-infinity-core-sit7.yml`,
    created_at: NOW - 12 * MIN,
    expires_at: NOW + 24 * HOUR - 12 * MIN,
    expected: { state: "hash", hash: hash("a91f3c0e") },
    new_hash: hash("7c3e55d1"),
    hunks: [
      hunk(42, 42, [ctx("  retention: 14d"), ctx("  timeoutMs: 8000"), del("  maxRetries: 3"), add("  maxRetries: 5"), ctx("  backoffMs: 250")]),
      hunk(61, 61, [del("  legacyAdapter: true"), ctx("  maskPatterns:")]),
    ],
    setting_changes: [change("tx-infinity-core.maxRetries", "3", "5", 44, "medium")],
  }),
  proposal({
    id: 2045,
    author: DANA,
    action: "delete",
    swimlane: "sit",
    path: "batch/legacy-batch-sit1.yml",
    created_at: NOW - 38 * MIN,
    expires_at: NOW + 24 * HOUR - 38 * MIN,
    expected: { state: "hash", hash: hash("77d2e0b4") },
    new_hash: ZERO,
  }),
  proposal({
    id: 2044,
    author: PRIYA,
    action: "pr",
    swimlane: "sitb",
    path: `${T}/tx-infinity-gateway-sit7.yml`,
    created_at: NOW - 80 * MIN,
    expires_at: NOW + 24 * HOUR - 80 * MIN,
    expected: { state: "hash", hash: hash("9a10b6c2") },
    new_hash: hash("4c2d8e9f"),
    hunks: [hunk(21, 21, [ctx("rateLimit:"), del("  rateLimitPerMin: 600"), add("  rateLimitPerMin: 1200"), ctx("  burst: 40")])],
    paths: [`${T}/tx-infinity-gateway-sit7.yml`],
  }),
  proposal({
    id: 2043,
    author: ANA,
    action: "restart",
    swimlane: "sitb",
    path: "sitb/gateway",
    paths: ["sitb/gateway", "sitb/tx-infinity-core"],
    created_at: NOW - 3 * HOUR,
    expires_at: NOW + 24 * HOUR - 3 * HOUR,
    expected: { state: "absent" },
    new_hash: ZERO,
  }),
  proposal({
    id: 2040,
    author: TOMAS,
    action: "upload",
    swimlane: "presita",
    path: "resources/receipt-templates-sit11.yml",
    created_at: NOW - 19 * HOUR,
    expires_at: NOW + 5 * HOUR,
    expected: { state: "hash", hash: hash("a5d03c8e") },
    new_hash: hash("c07a91e2"),
    hunks: [hunk(88, 88, [ctx("  footer:"), del("    text: Thank you"), add("    text: Thank you for banking with us"), add("    showQr: true")])],
  }),
];

export const STALE: readonly Proposal[] = [
  proposal({
    id: 2042,
    author: AMIR,
    status: "stale",
    swimlane: "sit",
    path: `${T}/payment-limits-sit1.yml`,
    created_at: NOW - 3 * HOUR,
    expires_at: NOW + 24 * HOUR - 3 * HOUR,
    expected: { state: "hash", hash: hash("77c0e1ab") },
    new_hash: hash("1d93aa07"),
    hunks: [hunk(12, 12, [ctx("limits:"), del("  perTxCents: 1000000"), add("  perTxCents: 1500000"), ctx("  daily: 5")])],
  }),
  proposal({
    id: 2041,
    author: LENA,
    status: "stale",
    swimlane: "sitc",
    path: `${T}/entryGroupConfig-sit9.yml`,
    created_at: NOW - 20 * HOUR,
    expires_at: NOW + 4 * HOUR,
    expected: { state: "hash", hash: hash("b71c04d9") },
    new_hash: hash("0d6e92b1"),
    hunks: [hunk(212, 212, [ctx("  atm:"), ctx("    DEPOSIT:"), ctx("      limitCents: 500000"), add("    WITHDRAWAL:")])],
  }),
];

export const EXPIRED: readonly Proposal[] = [
  proposal({
    id: 2037,
    author: TOMAS,
    status: "expired",
    action: "upload",
    swimlane: "presitc",
    path: "resources/receipt-templates-sit13.yml",
    created_at: NOW - 30 * HOUR,
    expires_at: NOW - 6 * HOUR,
    expected: { state: "hash", hash: hash("9be41f07") },
    new_hash: hash("9be41f07"),
  }),
  proposal({
    id: 2033,
    author: ANA,
    status: "expired",
    action: "delete",
    swimlane: "sitb",
    path: `${T}/tmp-hotfix.yml`,
    created_at: NOW - 2 * DAY,
    expires_at: NOW - 1 * DAY,
    expected: { state: "hash", hash: hash("18181818") },
    new_hash: ZERO,
  }),
];

export const DECIDED: readonly Proposal[] = [
  proposal({
    id: 2039,
    author: ANA,
    status: "applied",
    swimlane: "sitb",
    path: `${T}/account-sorting-config-sit7.yml`,
    created_at: NOW - 40 * MIN,
    expires_at: NOW + 23 * HOUR,
    decided_by: PRIYA,
    decided_at: NOW - 2 * MIN,
    expected: { state: "hash", hash: hash("3a77c1e0") },
    new_hash: hash("8b02d5f9"),
    hunks: [hunk(3, 3, [ctx("accountSorting:"), del("  enabled: false"), add("  enabled: true"), ctx("  sortOrder: asc")])],
  }),
  proposal({
    id: 2038,
    author: WEN,
    via: "mcp",
    status: "applied",
    swimlane: "sitc",
    path: `${T}/gateway-routes-sit9.yml`,
    created_at: NOW - 45 * MIN,
    expires_at: NOW + 23 * HOUR,
    decided_by: AMIR,
    decided_at: NOW - 37 * MIN,
    expected: { state: "hash", hash: hash("5b20e1aa") },
    new_hash: hash("d84c07f3"),
    hunks: [hunk(41, 41, [del("→ path: /accounts/**"), add("    path: /accounts/**")])],
  }),
  proposal({
    id: 2032,
    author: LENA,
    status: "rejected",
    swimlane: "sitb",
    path: "kafka/kafka-topics-sit7.yml",
    created_at: NOW - 9 * DAY,
    expires_at: NOW - 8 * DAY,
    decided_by: PRIYA,
    decided_at: NOW - 9 * DAY + 32 * MIN,
    decision_comment: "Wrong swimlane. The topic belongs on sitc.",
    expected: { state: "hash", hash: hash("1c80ab44") },
    new_hash: hash("66e2d0f1"),
    hunks: [hunk(40, 40, [ctx("  payments.dead:"), add("  payments.retry:"), add("    partitions: 3"), add("    replication: 3")])],
  }),
];

/** Every proposal of the page, by id. */
export const ALL: readonly Proposal[] = [...PENDING, ...STALE, ...EXPIRED, ...DECIDED];

// ---- the audit log (`Lanekeeper Audit Page.dc.html`) ----

const at = (hour: number, minute: number, second: number, day = 9): number => Date.UTC(2025, 9, day, hour, minute, second);
/** The chain hash of event `n`: a fixed function of the number, so that the chain is whole unless a test breaks it. */
export const chainHash = (n: number): string => n.toString(16).padStart(64, "0");

type Make2 = Partial<AuditEvent> & Pick<AuditEvent, "seq" | "at" | "action" | "actor" | "via">;

/** An audit event whose previous hash is the hash of the one before it. */
export function auditEvent(make: Make2): AuditEvent {
  return {
    id: make.seq,
    swimlane: "sitb",
    target: `${T}/tx-infinity-core-sit7.yml`,
    request_id: `req_${make.seq}`,
    attribution: null,
    diff: null,
    prev_hash: chainHash(make.seq - 1),
    hash: chainHash(make.seq),
    ...make,
  };
}

const lk = (user: string, confidence: Attribution["confidence"] = "certain"): Attribution => ({ source: "tool_write", confidence, actor: { kind: "user", user }, evidence: {} });
const unknown: Attribution = { source: "unknown", confidence: "low", actor: null, evidence: {} };

const DIFF_TMP = "--- /dev/null\n+++ b/tx-infinity-api/tmp-hotfix.yml\n@@ -0,0 +1,4 @@\n+# TEMP hotfix INC-2210\n+rateLimit:\n+  perClientPerMin: 1200\n+  burst: 80\n";
const DIFF_LIMITS = "--- a/tx-infinity-api/payment-limits-sit1.yml\n+++ b/tx-infinity-api/payment-limits-sit1.yml\n@@ -12,3 +12,3 @@ limits\n limits:\n-  perTxCents: 1000000\n+  perTxCents: 1500000\n   daily: 5\n";
const DIFF_SORT = "--- a/tx-infinity-api/account-sorting-config-sit7.yml\n+++ b/tx-infinity-api/account-sorting-config-sit7.yml\n@@ -3,3 +3,3 @@ accountSorting\n accountSorting:\n-  enabled: false\n+  enabled: true\n   sortOrder: asc\n";

/** The events the audit page draws, newest first (18,463 down to 18,450). Every one stores the hash of the one before it. */
export const AUDIT: readonly AuditEvent[] = [
  auditEvent({ seq: 18463, at: at(14, 44, 2), action: "flagged_value_revealed", actor: PRIYA, via: "ui", swimlane: "sitc", target: `${T}/tx-infinity-payments-sit9.yml`, attribution: lk(PRIYA) }),
  auditEvent({ seq: 18462, at: at(14, 41, 9), action: "file_edited", actor: PRIYA, via: "ui", target: `${T}/account-sorting-config-sit7.yml`, attribution: lk(ANA), diff: DIFF_SORT }),
  auditEvent({ seq: 18461, at: at(14, 41, 2), action: "proposal_approved", actor: PRIYA, via: "ui", target: `${T}/account-sorting-config-sit7.yml`, attribution: lk(PRIYA) }),
  auditEvent({ seq: 18460, at: at(14, 12, 51), action: "file_edited", actor: WEN, via: "mcp", swimlane: "sitc", target: `${T}/gateway-routes-sit9.yml`, attribution: lk(WEN) }),
  auditEvent({ seq: 18459, at: at(13, 58, 20), action: "file_change_detected", actor: null, via: "system", target: `${T}/tmp-hotfix.yml`, attribution: unknown, diff: DIFF_TMP }),
  auditEvent({ seq: 18458, at: at(13, 31, 44), action: "service_restarted", actor: DANA, via: "ui", target: "sitb/gateway", attribution: lk(DANA) }),
  auditEvent({ seq: 18457, at: at(13, 12, 10), action: "file_uploaded", actor: TOMAS, via: "ui", swimlane: "presita", target: "kafka/kafka-topics-sit11.yml", attribution: lk(TOMAS) }),
  auditEvent({ seq: 18456, at: at(12, 47, 33), action: "file_deleted", actor: LENA, via: "ui", swimlane: "sit", target: "batch/retired-feeds-sit1.yml", attribution: lk(LENA) }),
  auditEvent({ seq: 18455, at: at(12, 20, 5), action: "pr_raised", actor: AMIR, via: "ui", target: `${T}/tx-infinity-gateway-sit7.yml`, attribution: lk(AMIR) }),
  auditEvent({
    seq: 18454,
    at: at(11, 9, 30),
    action: "attribution_upgraded",
    actor: null,
    via: "system",
    swimlane: "sit",
    target: `${T}/payment-limits-sit1.yml`,
    attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "user", user: DANA }, evidence: { login_identity: "dana_okoye_corp_example", executable: "/usr/bin/vim.basic payment-limits-sit1.yml", observed_at: at(11, 4, 52) } },
  }),
  auditEvent({ seq: 18453, at: at(11, 5, 17), action: "file_change_detected", actor: null, via: "system", swimlane: "sit", target: `${T}/payment-limits-sit1.yml`, attribution: unknown, diff: DIFF_LIMITS }),
  auditEvent({
    seq: 18452,
    at: at(10, 42, 18),
    action: "file_change_detected",
    actor: null,
    via: "system",
    swimlane: "sitc",
    target: "resources/receipt-templates-sit9.yml",
    attribution: { source: "nfs_client", confidence: "medium", actor: { kind: "user", user: TOMAS }, evidence: { observed_at: at(10, 42, 18) } },
  }),
  auditEvent({
    seq: 18451,
    at: at(10, 40, 3),
    action: "file_edited",
    actor: null,
    via: "system",
    target: `${T}/tx-infinity-gateway-sit7.yml`,
    attribution: { source: "sync_job", confidence: "certain", actor: { kind: "job", job: { name: "sit7-sync", uid: "7731" } }, evidence: {} },
  }),
  auditEvent({ seq: 18450, at: at(10, 12, 40), action: "file_edited", actor: DANA, via: "ui", target: `${T}/tx-infinity-gateway-sit7.yml`, attribution: lk(DANA) }),
];
