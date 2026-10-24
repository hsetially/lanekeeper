// Builders and sample data of the four Admin sections that task T5 adds (sentinels, OS Login mapping, severity rules, retention). They
// follow the Admin page of the handoff (`Lanekeeper Admin Page.dc.html`, boards 1ao to 1bm) where the contract has the data for it,
// and the contract where it does not (see design/DEVIATIONS.md). The scenarios of this folder and the tests of the sections both use
// them, so a shape is written once and is valid against the contract.
// Times come from the frozen clock the visual runs use (1_760_000_000_000).
// Loaded by Node as well as Vite: relative imports with the `.ts` extension, `import type`, nothing that cannot be erased.
import type { OsLoginUser, RetentionPreview, RetentionSettings, SentinelInfo, SeverityRule } from "../../../api/generated/model/index.ts";
import { DAY, HOUR, IDS, MINUTE, NOW } from "../admin/fixtures.ts";

export { DAY, HOUR, IDS, MINUTE, NOW };

const GIB = 1024 ** 3;

/** One sentinel as the Admin list returns it. */
export function sentinel(host: string, over: Partial<SentinelInfo> = {}): SentinelInfo {
  return { id: host, host, version: "v0.9.4", status: "connected", last_seen_at: NOW - 8_000, spool_backlog: 0, ...over };
}

/**
 * The six NFS VMs of the design (one for each swimlane). `nfs-presitb-01` is disconnected and was last seen 14 minutes ago (the design's
 * heartbeat gap), `nfs-sitc-01` runs an older version and `nfs-sitb-01` has records waiting to be delivered.
 */
export function sentinels(): SentinelInfo[] {
  return [
    sentinel("nfs-sit-01", { last_seen_at: NOW - 4_000 }),
    sentinel("nfs-sitb-01", { last_seen_at: NOW - 11_000, spool_backlog: 2 }),
    sentinel("nfs-sitc-01", { version: "v0.9.3", last_seen_at: NOW - 40_000 }),
    sentinel("nfs-presita-01", { last_seen_at: NOW - 6_000 }),
    sentinel("nfs-presitb-01", { status: "disconnected", last_seen_at: NOW - 14 * MINUTE, spool_backlog: 38 }),
    sentinel("nfs-presitc-01", { last_seen_at: NOW - 3_000 }),
  ];
}

/** The admin's manual mappings: a contractor whose OS Login name does not follow from their email, and a second account of Lena's. */
export function osLoginUsers(): OsLoginUser[] {
  return [
    { os_login: "marco_diaz_contractor_example", user: IDS.marco },
    { os_login: "lena_b_corp_example", user: IDS.lena },
  ];
}

/** A rule of the admin's own. Built-in rules are not in the list the hub returns. */
export function severityRule(id: string, over: Partial<SeverityRule> = {}): SeverityRule {
  return { id, name: id, match: {}, severity: "high", priority: 100, enabled: true, ...over };
}

export function severityRules(): SeverityRule[] {
  return [
    severityRule("security-high", { name: "Security files are High", match: { path_glob: "security/**" }, severity: "high", priority: 100 }),
    severityRule("prod-host-critical", { name: "A prod host in a SIT file is Critical", match: { finding_kinds: ["environment_host_mismatch"], path_glob: "services/**" }, severity: "critical", priority: 200 }),
    severityRule("removed-settings", { name: "A removed setting is Medium", match: { change_kinds: ["removed", "type_changed"] }, severity: "medium", priority: 300 }),
    severityRule("payments-high", { name: "Payment limits are High", match: { path_glob: "services/payment-limits/**" }, severity: "high", priority: 400, enabled: false }),
  ];
}

/**
 * What the retention job would delete for a policy: a smooth function of the two numbers (the design's model, 41,208 versions at 180
 * days and 50 per file), so a test and a scenario can tell two policies apart. The real answer is the engine's.
 */
export function previewFor(keepDays: number, keepLast: number): RetentionPreview {
  const versions = Math.min(380_000, Math.round(41_208 * (180 / keepDays) ** 0.9 * (50 / keepLast) ** 0.5));
  return { versions_to_delete: versions, bytes_to_free: Math.round((3.2 * GIB * versions) / 41_208), versions_kept_by_reference: Math.round(318 * (180 / keepDays) ** 0.5) };
}

/** The saved policy: the contract's defaults (180 days, 50 per file), changed by Priya Shah 40 days ago, with its dry run. */
export function retention(over: Partial<RetentionSettings> = {}): RetentionSettings {
  return { keep_last_versions: 50, keep_days: 180, updated_at: NOW - 40 * DAY, updated_by: IDS.priya, dry_run_preview: previewFor(180, 50), ...over };
}
