// Builders and sample data of the Admin screens (S8): the people, the access request, the agent registrations and the rules that
// the Admin page of the handoff draws (`Lanekeeper Admin Page.dc.html`). The scenarios of this folder and the tests of
// src/features/admin both use them, so a shape is written once and is valid against the contract (src/mocks/mocks.test.ts checks
// the scenarios).
// Times come from the frozen clock the visual runs use (1_760_000_000_000), so "2 h ago" is the same on every run.
// Loaded by Node as well as Vite: relative imports with the `.ts` extension, `import type`, nothing that cannot be erased.
import type {
  AccessRequest,
  AgentRegistration,
  FolderMapping,
  HostTier,
  McpToken,
  MergeModeRule,
  NotificationChannel,
  PathMappingRule,
  User,
} from "../../../api/generated/model/index.ts";

export const NOW = 1_760_000_000_000;
export const MINUTE = 60_000;
export const HOUR = 60 * MINUTE;
export const DAY = 24 * HOUR;

const TENANT = "11111111-1111-4111-8111-111111111111";
const uid = (oid: string): string => `${TENANT}:${oid}`;

export const IDS = {
  priya: uid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
  marco: uid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
  amir: uid("cccccccc-cccc-4ccc-8ccc-cccccccccccc"),
  lena: uid("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
  dana: uid("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"),
  tomas: uid("ffffffff-ffff-4fff-8fff-ffffffffffff"),
  ines: uid("99999999-9999-4999-8999-999999999999"),
  sam: uid("12121212-1212-4121-8121-121212121212"),
} as const;

/** A user as the Admin list returns it. `over` replaces any field. */
export function person(id: string, display_name: string, over: Partial<User> = {}): User {
  const email = `${display_name.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/ /g, ".")}@corp.example`;
  return { id, email, display_name, role: "editor", status: "active", requires_approval: false, github_login: null, last_seen_at: NOW - 2 * HOUR, ...over };
}

/** The seven people of the Admin page. The first is the signed-in Admin. */
export function people(): User[] {
  return [
    person(IDS.priya, "Priya Shah", { role: "admin", github_login: "priyashah", last_seen_at: NOW }),
    person(IDS.amir, "Amir Haddad", { role: "admin", github_login: "ahaddad", last_seen_at: NOW - 14 * MINUTE }),
    person(IDS.lena, "Lena Brandt", { role: "editor", github_login: "lbrandt", last_seen_at: NOW - 2 * HOUR }),
    person(IDS.dana, "Dana Okoye", { role: "editor", requires_approval: true, github_login: "dokoye", last_seen_at: NOW - DAY }),
    person(IDS.marco, "Marco Diaz", { role: "editor", requires_approval: true, github_login: "mdiaz-c", last_seen_at: NOW - 9 * DAY }),
    person(IDS.tomas, "Tomás Reyes", { role: "viewer", github_login: null, last_seen_at: NOW - 10 * DAY }),
    person(IDS.ines, "Ines Costa", { role: "viewer", status: "disabled", github_login: "icosta", last_seen_at: NOW - 35 * DAY }),
  ];
}

/** Sam Okafor's request, as the Admin page's first frame shows it. */
export function samRequest(over: Partial<AccessRequest> = {}): AccessRequest {
  return {
    id: 41,
    user: IDS.sam,
    email: "sam.okafor@corp.example",
    requested_role: "editor",
    reason: "Joining the sit swimlane team. I need to edit sitb configs.",
    status: "pending",
    created_at: NOW - 2 * HOUR,
    decided_by: null,
    decided_at: null,
    ...over,
  };
}

const SA = (project: string): string => `lanekeeper-agent@${project}.iam.gserviceaccount.com`;

/** The agent registration of a swimlane. */
export function registration(swimlane: string, over: Partial<AgentRegistration> = {}): AgentRegistration {
  const pre = swimlane.startsWith("presit");
  return {
    swimlane,
    service_account_email: SA(pre ? "dfb-nonprod-presit" : "dfb-nonprod-sit"),
    status: "connected",
    connected_replica: `lk-agent-${swimlane}-x2kqz`,
    last_seen_at: NOW - 4_000,
    certificate_expires_at: NOW + 42 * DAY,
    ...over,
  };
}

/** The registrations of the design's six swimlanes: presitb is offline, presitc has never been seen. */
export function registrations(): AgentRegistration[] {
  return [
    registration("sit"),
    registration("sitb", { last_seen_at: NOW - 52_000 }),
    registration("sitc", { certificate_expires_at: NOW + 10 * DAY }),
    registration("presita", { certificate_expires_at: NOW + 8 * DAY }),
    registration("presitb", { status: "disconnected", connected_replica: null, last_seen_at: NOW - 2 * HOUR - 14 * MINUTE }),
    registration("presitc", { status: "never_seen", connected_replica: null, last_seen_at: null, certificate_expires_at: null }),
  ];
}

export function pathRules(): PathMappingRule[] {
  return [
    { id: "default", base_root: "config", tenant_root: "data/config", swimlanes: [] },
    { id: "presit-legacy", base_root: "services", tenant_root: "services-tenant", swimlanes: ["presita", "presitb"] },
  ];
}

export function mergeRules(): MergeModeRule[] {
  return [
    { id: "security", file_pattern: "security/**", mode: "spring_merge" },
    { id: "services", file_pattern: "services/**/*.yml", mode: "whole_file" },
    { id: "images", file_pattern: "**/*.png", mode: "whole_file" },
  ];
}

export function hostTiers(): HostTier[] {
  return [
    { id: "dev", host_pattern: "*-dev.example.net", tier: "dev" },
    { id: "uat", host_pattern: "*-uat.example.net", tier: "uat" },
    { id: "prod", host_pattern: "*.prod.example.net", tier: "prod" },
  ];
}

export function folders(): FolderMapping[] {
  return [
    { id: "f-video", folder: "video", deployments: [{ namespace: "sitb", name: "video-service" }] },
    { id: "f-tx-infinity-api", folder: "tx-infinity-api", deployments: [{ namespace: "sitb", name: "tx-infinity-core" }, { namespace: "sitb", name: "tx-infinity-gateway" }] },
    { id: "f-security", folder: "security", deployments: [{ namespace: "sitb", name: "security-roles" }] },
  ];
}

export function channels(): NotificationChannel[] {
  return [
    { id: "certificates", name: "Certificate expiring", webhook_configured: true, min_severity: "medium", kinds: ["agent_disconnected", "sync_window_stale"], enabled: true },
    { id: "approvals", name: "Approvals", webhook_configured: true, min_severity: "low", kinds: ["proposal_pending", "proposal_decided", "access_requested"], enabled: true },
    { id: "findings", name: "Critical findings", webhook_configured: false, min_severity: "critical", kinds: ["critical_finding"], enabled: false },
  ];
}

/** A personal MCP token of the signed-in Admin, as the list returns it. */
export function mcpToken(id: number, label: string, over: Partial<McpToken> = {}): McpToken {
  return { id, label, created_at: NOW - 2 * DAY, expires_at: NOW + 5 * DAY, revoked_at: null, last_used_at: NOW - HOUR, ...over };
}
