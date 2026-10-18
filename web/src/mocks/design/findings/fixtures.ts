// Builders and data for the Findings screen: a finding that is valid against the contract, one example of every kind of the
// contract (C1 to C12), and the eleven open findings of the Findings board of the design (`Lanekeeper Findings Page.dc.html`),
// restated in what the contract carries. The scenarios of this folder and the tests of src/features/findings use them.
//
// Where the design draws more than the contract gives a finding (commits, hashes, a timeline, who marked it intentional), the
// fixture gives what `Finding` carries and no more: an id, a kind, a swimlane, a path, a severity, the rules that rated it, one
// line of text and the places in the file. Two of the design's checks mean something else in the contract (the design's C3
// "override has no base file" and C5 "file does not parse"), so the examples below follow the contract's checks.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`, nothing that
// cannot be erased.
import type { Finding, FindingKind, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import { SHELL_LANES } from "../shell/index.ts";

/** The frozen clock the visual runs use. */
export const NOW = 1_760_000_000_000;
const MIN = 60_000;
const HOUR = 60 * MIN;

/** A finding: a drift on a file of sitb, medium, found a minute ago. `over` replaces any field. */
export function finding(id: number, over: Partial<Finding> = {}): Finding {
  return {
    id,
    kind: "drift",
    swimlane: "sitb",
    path: "tx-infinity-api/tx-infinity-core-sit7.yml",
    severity: "medium",
    rule_ids: [],
    message: "The file differs between NFS and Git.",
    locations: [],
    detected_at: NOW - MIN,
    ...over,
  };
}

const at = (setting: string, start: number, end = start) => ({ path: setting, lines: { start, end } });

/** The eleven open findings of the design's Findings page, in the order of the contract's list (newest first, by id). */
export const DESIGN_FINDINGS: readonly Finding[] = [
  finding(11, {
    kind: "duplicate_keys",
    swimlane: "presitb",
    path: "security/security-roles.yml",
    severity: "high",
    rule_ids: ["default.duplicate-keys"],
    message: "Duplicate keys in security-roles.yml at lines 5171, 5643 and 6115",
    locations: [at("roles.teller-supervisor", 5171), at("roles.teller-supervisor", 5643), at("roles.teller-supervisor", 6115)],
    detected_at: NOW - 5 * MIN,
  }),
  finding(10, {
    kind: "overwritten_nfs_change",
    swimlane: "sitb",
    path: "tx-infinity-api/tx-infinity-gateway-sit7.yml",
    severity: "high",
    rule_ids: ["default.overwritten-nfs-change"],
    message: "A sync overwrote an NFS change to rateLimitPerMin",
    locations: [at("rateLimit.rateLimitPerMin", 22)],
    logical_file: "tx-infinity-api/tx-infinity-gateway.yml",
    tenant: "sit7",
    detected_at: NOW - 21 * MIN,
  }),
  finding(9, {
    kind: "environment_host_mismatch",
    swimlane: "presitb",
    path: "tx-infinity-api/tx-infinity-payments-sit12.yml",
    severity: "high",
    rule_ids: ["default.environment-host"],
    message: "presitb sends payment callbacks to a dev host",
    locations: [at("callbacks.url", 57)],
    logical_file: "tx-infinity-api/tx-infinity-payments.yml",
    tenant: "sit12",
    detected_at: NOW - 40 * MIN,
  }),
  finding(8, {
    kind: "environment_host_mismatch",
    swimlane: "presita",
    path: "tx-infinity-api/tx-infinity-core-sit11.yml",
    severity: "high",
    rule_ids: ["default.environment-host"],
    message: "presita references a dev host",
    locations: [at("kafka.bootstrap", 88)],
    logical_file: "tx-infinity-api/tx-infinity-core.yml",
    tenant: "sit11",
    detected_at: NOW - HOUR,
  }),
  finding(7, {
    kind: "drift",
    swimlane: "sitc",
    path: "tx-infinity-api/gateway-routes-sit9.yml",
    severity: "high",
    rule_ids: ["default.conflict"],
    message: "gateway-routes-sit9.yml changed on NFS and in Git, in different ways",
    locations: [],
    logical_file: "tx-infinity-api/gateway-routes.yml",
    tenant: "sit9",
    detected_at: NOW - 2 * HOUR,
  }),
  finding(6, {
    kind: "orphan_file",
    swimlane: "sitb",
    path: "tx-infinity-api/tmp-hotfix.yml",
    severity: "medium",
    rule_ids: ["default.out-of-band"],
    message: "tmp-hotfix.yml exists on sitb's NFS only; Git and base have no copy",
    locations: [at("(whole file)", 1, 18)],
    detected_at: NOW - 2 * HOUR,
  }),
  finding(4, {
    kind: "redundant_tenant_file",
    swimlane: "sitc",
    path: "resources/receipt-templates-sit9.yml",
    severity: "low",
    rule_ids: [],
    message: "receipt-templates-sit9.yml is identical to base and blocks future base changes",
    locations: [at("(whole file)", 1, 212)],
    logical_file: "resources/receipt-templates.yml",
    tenant: "sit9",
    detected_at: NOW - 3 * HOUR,
  }),
  finding(5, {
    kind: "uneven_tenant_files",
    swimlane: "sit",
    path: "batch/legacy-batch-sit1.yml",
    severity: "low",
    rule_ids: [],
    message: "legacy-batch-sit1.yml has no counterpart for the other tenants of the swimlane",
    locations: [at("(whole file)", 1, 38)],
    logical_file: "batch/legacy-batch.yml",
    tenant: "sit1",
    detected_at: NOW - 4 * HOUR,
  }),
  finding(3, {
    kind: "redundant_tenant_file",
    swimlane: "sitb",
    path: "tx-infinity-api/account-sorting-config-sit7.yml",
    severity: "medium",
    rule_ids: [],
    message: "account-sorting-config-sit7.yml is identical to base and blocks future base changes",
    locations: [at("(whole file)", 1, 64)],
    logical_file: "tx-infinity-api/account-sorting-config.yml",
    tenant: "sit7",
    detected_at: NOW - 5 * HOUR,
  }),
  finding(2, {
    kind: "missed_base_changes",
    swimlane: "presita",
    path: "kafka/kafka-topics-sit11.yml",
    severity: "medium",
    rule_ids: ["default.missed-base-changes"],
    message: "presita's tenant copy of kafka-topics.yml has no payments.retry topic that base has",
    locations: [at("kafka.topics.payments.retry", 40, 46)],
    logical_file: "kafka/kafka-topics.yml",
    tenant: "sit11",
    detected_at: NOW - 6 * HOUR,
  }),
  finding(1, {
    kind: "missed_base_changes",
    swimlane: "sitc",
    path: "tx-infinity-api/entryGroupConfig-sit9.yml",
    severity: "medium",
    rule_ids: ["default.missed-base-changes"],
    message: "sitc's tenant copy of entryGroupConfig.yml has no atm entry that base has",
    locations: [at("entryGroupConfigMap.atm", 209, 217)],
    logical_file: "tx-infinity-api/entryGroupConfig.yml",
    tenant: "sit9",
    detected_at: NOW - 7 * HOUR,
  }),
].sort((a, b) => b.id - a.id);

/** One finding of every kind of the contract, C1 to C12, for the tests and the "all kinds" scenario. Ids run 101 to 112. */
export const KIND_EXAMPLES: Readonly<Record<FindingKind, Finding>> = {
  missed_base_changes: finding(101, { kind: "missed_base_changes", severity: "medium", message: "The tenant copy has no atm entry that base has", locations: [at("entryGroupConfigMap.atm", 209, 217)] }),
  redundant_tenant_file: finding(102, { kind: "redundant_tenant_file", severity: "low", message: "The tenant file is identical to base", locations: [at("(whole file)", 1, 64)] }),
  uneven_tenant_files: finding(103, { kind: "uneven_tenant_files", severity: "low", message: "sit7 has its own copy of this file and sit8 does not", locations: [] }),
  orphan_file: finding(104, { kind: "orphan_file", severity: "medium", path: "tx-infinity-api/tmp-hotfix.yml", message: "The file is on NFS only", locations: [] }),
  drift: finding(105, { kind: "drift", severity: "medium", message: "The file differs between NFS and Git.", locations: [] }),
  environment_host_mismatch: finding(106, { kind: "environment_host_mismatch", severity: "high", message: "kafka-dev3.corp.internal is a dev host in a pre-production swimlane", locations: [at("kafka.bootstrap", 88)] }),
  overwritten_nfs_change: finding(107, { kind: "overwritten_nfs_change", severity: "critical", message: "A sync overwrote an NFS change to rateLimitPerMin", locations: [at("rateLimit.rateLimitPerMin", 22)] }),
  duplicate_keys: finding(108, { kind: "duplicate_keys", severity: "low", path: "security/security-roles.yml", message: "roles.txRemoteSupervisor appears three times", locations: [at("roles.txRemoteSupervisor", 6), at("roles.txRemoteSupervisor", 12), at("roles.txRemoteSupervisor", 14)] }),
  ambiguous_file_name: finding(109, { kind: "ambiguous_file_name", severity: "low", path: "ui/remote-itm-teller/authentication-config.yaml", message: "authentication-config.yaml is in 12 folders", locations: [] }),
  unresolved_placeholder: finding(110, { kind: "unresolved_placeholder", severity: "medium", message: "${KAFKA_REPLICAS} is neither a setting nor an environment variable of any service", locations: [at("kafka.replicas", 31)] }),
  config_server_restart_required: finding(111, { kind: "config_server_restart_required", severity: "medium", path: null, message: "channels.yml changed after the config-server started", locations: [] }),
  served_mismatch: finding(112, { kind: "served_mismatch", severity: "high", rule_ids: ["registry.served_mismatch"], message: "The config-server serves different content for this file", locations: [] }),
};

/** The Shell's six swimlanes, with the finding counts the Findings page adds up to (11), and every agent online. */
export function laneCounts(findings: readonly Finding[], over: Partial<Record<string, Partial<SwimlaneSummary>>> = {}): SwimlaneSummary[] {
  return SHELL_LANES.map((lane) => ({
    ...lane,
    agent: "connected",
    last_scan_at: NOW - 4 * MIN,
    finding_count: findings.filter((f) => f.swimlane === lane.id).length,
    ...over[lane.id],
  }));
}
