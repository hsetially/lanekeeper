// The data of the attribution scenarios (T5, slice A): one file's history, and one audit log, in which every source of D73 and
// every level of confidence appears, with people the Admin's user list names and people it does not. Synthetic, made up for the
// design; nothing here is a real tenant's configuration or a real person. The hostile variants carry the text an NFS VM or a
// cluster can send: a right-to-left override in an OS login, a zero-width space in a job name and in an executable.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { Attribution, AuditEvent, User, Version } from "../../../api/generated/model/index.ts";
import { chainHash } from "../approvals/fixtures.ts";
import { hash, NOW } from "../file/data.ts";
import { MARCO, PRIYA } from "../shell/index.ts";

export { MARCO, PRIYA };

const MIN = 60_000;
const HOUR = 60 * MIN;

/** A person the Admin's user list knows, and one it does not (a user who was removed): the second is drawn by the start of the id. */
export const GONE = "11111111-1111-4111-8111-111111111111:cccccccc-cccc-4ccc-8ccc-cccccccccccc";

export const USERS: readonly User[] = [
  { id: PRIYA, email: "priya.shah@example.test", display_name: "Priya Shah", role: "admin", status: "active", requires_approval: false, github_login: null, last_seen_at: NOW - HOUR },
  { id: MARCO, email: "marco.diaz@example.test", display_name: "Marco Diaz", role: "editor", status: "active", requires_approval: false, github_login: null, last_seen_at: NOW - 2 * HOUR },
];

const user = (id: string, confidence: Attribution["confidence"] = "certain", evidence: Attribution["evidence"] = {}): Attribution => ({ source: "tool_write", confidence, actor: { kind: "user", user: id }, evidence });

/** The six sources, one attribution each. */
export const BY_SOURCE = {
  tool: user(MARCO),
  login: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" }, evidence: { executable: "vi", login_identity: "lkowalski", observed_at: NOW - 5 * HOUR } },
  mapped: { source: "sentinel_login", confidence: "high", actor: { kind: "user", user: MARCO }, evidence: { executable: "vim", observed_at: NOW - 6 * HOUR } },
  sync: { source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp-tenant-data-sit7", uid: "7d1c0b5e-4a1b-4c8e-9f0a-2b3c4d5e6f70" } }, evidence: {} },
  client: { source: "nfs_client", confidence: "medium", actor: null, evidence: { executable: "vim" } },
  owner: { source: "fs_owner_hint", confidence: "low", actor: { kind: "os_login", login: "svc-batch" }, evidence: {} },
  // The contract writes `low` here; D73 says none, and the app must not show a Low.
  unknown: { source: "unknown", confidence: "low", actor: null, evidence: {} },
} as const satisfies Record<string, Attribution>;

type Seed = { ago: number; source: Version["source"]; attribution: Attribution; severity: Version["severity"] };

function versions(seeds: readonly Seed[]): Version[] {
  return seeds.map((seed, i) => ({
    hash: hash(`a${i.toString(16)}7e22b`),
    observed_at: NOW - seed.ago,
    source: seed.source,
    size: 9_100 - i * 40,
    attribution: seed.attribution,
    severity: seed.severity,
    commit: seed.source === "sync" ? "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c" : null,
  }));
}

/** The history of a file, newest first: every source, every level, and every severity (and one version nobody rated). */
export function historyVersions(): Version[] {
  return versions([
    { ago: 5 * MIN, source: "scan", attribution: BY_SOURCE.unknown, severity: "high" },
    { ago: HOUR, source: "tool_write", attribution: user(PRIYA), severity: "low" },
    { ago: 2 * HOUR, source: "tool_write", attribution: BY_SOURCE.tool, severity: "low" },
    { ago: 3 * HOUR, source: "sync", attribution: BY_SOURCE.sync, severity: null },
    { ago: 5 * HOUR, source: "scan", attribution: BY_SOURCE.login, severity: "critical" },
    { ago: 6 * HOUR, source: "scan", attribution: BY_SOURCE.mapped, severity: "medium" },
    { ago: 26 * HOUR, source: "scan", attribution: BY_SOURCE.client, severity: "medium" },
    { ago: 30 * HOUR, source: "scan", attribution: BY_SOURCE.owner, severity: "low" },
    { ago: 40 * HOUR, source: "tool_write", attribution: user(GONE), severity: null },
  ]);
}

/** The same shape of history, with text an NFS VM or a cluster could send to fool a reader. */
export function hostileVersions(): Version[] {
  return versions([
    { ago: 5 * MIN, source: "scan", attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "invoice\u202egnp.exe" }, evidence: { executable: "v\u200di", login_identity: "do\u2066kye" } }, severity: "high" },
    { ago: HOUR, source: "sync", attribution: { source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp\u200b-tenant-data-sit7", uid: "u\u202e1" } }, evidence: {} }, severity: null },
    { ago: 2 * HOUR, source: "scan", attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "<img src=x onerror=alert(1)>" }, evidence: { executable: "<script>alert(1)</script>" } }, severity: "medium" },
    { ago: 3 * HOUR, source: "scan", attribution: { source: "fs_owner_hint", confidence: "low", actor: { kind: "os_login", login: "a".repeat(256) }, evidence: {} }, severity: "low" },
  ]);
}

const T = "tx-infinity-api";
const at = (ago: number): number => NOW - ago;

type Make = Partial<AuditEvent> & Pick<AuditEvent, "seq" | "at" | "action" | "actor" | "via">;
function event(make: Make): AuditEvent {
  return { id: make.seq, swimlane: "sitb", target: `${T}/tx-infinity-core-sit7.yml`, request_id: `req_${make.seq}`, attribution: null, diff: null, prev_hash: chainHash(make.seq - 1), hash: chainHash(make.seq), ...make };
}

/** The audit log, newest first: an event for each source and each level, a mapped user, a user nobody knows any more, and an event nobody made. */
export function auditEvents(): AuditEvent[] {
  return [
    event({ seq: 18470, at: at(2 * MIN), action: "file_edited", actor: PRIYA, via: "ui", attribution: user(PRIYA) }),
    event({ seq: 18469, at: at(9 * MIN), action: "file_edited", actor: PRIYA, via: "mcp", attribution: user(MARCO) }),
    event({ seq: 18468, at: at(20 * MIN), action: "file_change_detected", actor: null, via: "system", attribution: BY_SOURCE.login }),
    event({ seq: 18467, at: at(35 * MIN), action: "attribution_upgraded", actor: null, via: "system", attribution: BY_SOURCE.mapped }),
    event({ seq: 18466, at: at(HOUR), action: "file_change_detected", actor: null, via: "system", attribution: BY_SOURCE.sync }),
    event({ seq: 18465, at: at(2 * HOUR), action: "file_change_detected", actor: null, via: "system", attribution: BY_SOURCE.client }),
    event({ seq: 18464, at: at(3 * HOUR), action: "file_change_detected", actor: null, via: "system", attribution: BY_SOURCE.owner }),
    event({ seq: 18463, at: at(4 * HOUR), action: "file_change_detected", actor: null, via: "system", attribution: BY_SOURCE.unknown }),
    event({ seq: 18462, at: at(5 * HOUR), action: "file_edited", actor: GONE, via: "ui", attribution: user(GONE) }),
    event({ seq: 18461, at: at(6 * HOUR), action: "service_restarted", actor: PRIYA, via: "ui", target: "sitb/gateway", attribution: null }),
    event({ seq: 18460, at: at(7 * HOUR), action: "settings_changed", actor: null, via: "system", target: null, attribution: null }),
    event({
      seq: 18459,
      at: at(8 * HOUR),
      action: "file_change_detected",
      actor: null,
      via: "system",
      attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "invoice\u202egnp.exe" }, evidence: { executable: "v\u200di", login_identity: "do\u2066kye" } },
    }),
  ];
}
