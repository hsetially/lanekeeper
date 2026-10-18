// Builders for the swimlane screens' data: a summary, a detail with services, one service. The scenarios of this folder and the
// tests of src/features/swimlanes both use them, so the shapes are written once and every one of them is valid against the
// contract (src/mocks/mocks.test.ts checks the scenarios; a test that builds its own data builds it with these).
// Loaded by Node as well as Vite: relative imports with the `.ts` extension, `import type`, nothing that cannot be erased.
import type { AuditEvent, FileEntry, Finding, ServiceState, SwimlaneDetail, SwimlaneSummary } from "../../../api/generated/model/index.ts";

export const COMMIT = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";
export const PRIYA_ID = "11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

/** A swimlane summary: connected agent, confirmed baseline, no drift, no findings. `over` replaces any field. */
export function lane(id: string, over: Partial<SwimlaneSummary> = {}): SwimlaneSummary {
  return {
    id,
    display_name: id,
    tenants: ["sit1"],
    agent: "connected",
    last_scan_at: 1_760_000_000_000,
    base_version: { tag: "v2.3.1", distance: 0, commit: COMMIT },
    drift_counts: [],
    finding_count: 0,
    tier: "SIT",
    project: "ops-sit",
    baseline: { state: "confirmed", by: { id: PRIYA_ID, display_name: "Priya Shah" }, at: 1_759_000_000_000 },
    ...over,
  };
}

/** A service of a swimlane: namespace is the swimlane's, picked up (live) unless `over` says otherwise. */
export function service(name: string, over: Partial<ServiceState> = {}, namespace = "sitb"): ServiceState {
  return {
    service: { namespace, name },
    pickup: "live",
    pending_since: null,
    live_by: null,
    notifications_enabled: true,
    cache_ttl_secs: 300,
    pending_files: [],
    ...over,
  };
}

/** The detail of a swimlane: its summary, its services and its auto-notify setting. */
export function detailOf(summary: SwimlaneSummary, services: readonly ServiceState[] = [], autoNotify = false): SwimlaneDetail {
  return { ...summary, services: [...services], auto_notify: autoNotify };
}

/** A file of the swimlane, in sync, a structured property source. `over` replaces any field. */
export function fileEntry(path: string, over: Partial<FileEntry> = {}): FileEntry {
  return {
    path,
    is_dir: false,
    kind: "base",
    class: "structured",
    role: "property_source",
    size: 120,
    hash: "a".repeat(64),
    drift: "in_sync",
    format: { eol: "lf", encoding: "utf8", bom: false },
    content_withheld: false,
    attribution: null,
    severity: null,
    pickup_state: null,
    ...over,
  };
}

/** A folder of the swimlane. */
export function folderEntry(path: string, over: Partial<FileEntry> = {}): FileEntry {
  return fileEntry(path, { is_dir: true, class: "text", format: null, hash: null, size: 0, ...over });
}

/** A finding of a check on a swimlane. */
export function finding(id: number, over: Partial<Finding> = {}): Finding {
  return {
    id,
    kind: "duplicate_keys",
    swimlane: "sitb",
    path: "tx-infinity-api/tx-infinity-core-sit7.yml",
    severity: "medium",
    rule_ids: [],
    message: "The key enableAccountSorting is set twice.",
    locations: [{ path: "txInfinityOptions.enableAccountSorting", lines: { start: 212, end: 212 } }],
    detected_at: 1_760_000_000_000,
    ...over,
  };
}

/** A row of the audit trail. */
export function auditEvent(id: number, over: Partial<AuditEvent> = {}): AuditEvent {
  const hash = "b".repeat(64);
  return {
    id,
    seq: id,
    at: 1_760_000_000_000 + id * 1000,
    action: "file.edit",
    actor: PRIYA_ID,
    via: "ui",
    swimlane: "sitb",
    target: "tx-infinity-api/tx-infinity-core-sit7.yml",
    request_id: null,
    attribution: null,
    diff: null,
    prev_hash: hash,
    hash,
    ...over,
  };
}
