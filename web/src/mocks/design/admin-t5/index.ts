// The design-parity scenarios of the four Admin sections that task T5 adds: the data of the Admin page of the handoff
// (`Lanekeeper Admin Page.dc.html` and the board `Lanekeeper Admin`, frames 1ao to 1bm) as far as the contract carries it.
//
//  - The signed-in person is Priya Shah, an Admin, and the sidebar is the Shell page's (six swimlanes, 11 findings, 6 approvals): each
//    scenario starts from `design-shell-admin` and adds what the section asks for, as the S8 scenarios do. Every Admin screen asks for
//    the pending access requests (the count beside Users), so every scenario answers that.
//  - A state of a form (editing, a field in error) is not data: it is what a person does on the screen, so one scenario serves the
//    default, editing and error frames of a section. What differs in data is the empty list.
//  - The hub answers every write with the same body whatever was sent (a scenario is not a server), so a list does not change after a
//    write in the visual runs; the Playwright journeys that need that use a hub with state (e2e).
//  - The contract has no "empty" retention policy (the defaults always apply) and no sentinel threshold, so the design's frame for the
//    first (1be) is drawn as `retention-defaults` and the frames for the second (1aq to 1as) are not drawn (design/DEVIATIONS.md).
// Scenario ids are `design-admin-t5-<name>`, for the folder `admin-t5` (src/mocks/design/index.ts adds it to the slices).
// Times are derived from the frozen clock the visual runs use (e2e/visual/render-references.mjs: 1_760_000_000_000).
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";
import { scenarios as shell } from "../shell/index.ts";
import { people, samRequest } from "../admin/fixtures.ts";
import { IDS, osLoginUsers, previewFor, retention, sentinels, severityRules } from "./fixtures.ts";

const page = <T>(items: readonly T[]): { items: readonly T[]; next: null } => ({ items, next: null });
// The Shell's own answer to the pending requests is an empty list; the harness answers with the FIRST response that matches, so it
// must not be here or it hides the page's own.
const shellBase = (): readonly MockResponse[] => (shell.find((s) => s.id === "design-shell-admin")?.responses ?? []).filter((r) => !(r.method === "GET" && (r.path === "/api/v1/admin/access-requests" || r.path === "/api/v1/admin/users")));

const list = (path: string, items: readonly unknown[]): MockResponse => ({ method: "GET", path, body: page(items) });
const noContent = (path: string): MockResponse => ({ method: "DELETE", path, status: 204 });
const bodyOf = (c: MockContext): Record<string, unknown> => (typeof c.body === "object" && c.body !== null ? (c.body as Record<string, unknown>) : {});

function admin(id: string, description: string, ...own: MockResponse[]): Scenario {
  return { id, description, responses: [...shellBase(), list("/api/v1/admin/access-requests", [samRequest()]), ...own] };
}

const RULE_BODY = { name: "Security files are High", match: { path_glob: "security/**" }, severity: "high", priority: 100, enabled: true };

const OS_LOGIN_ANSWERS: readonly MockResponse[] = [
  list("/api/v1/admin/users", people()),
  {
    method: "PUT",
    path: "/api/v1/admin/os-login-users/{login}",
    body: (c: MockContext) => ({ os_login: c.params["login"] ?? "", user: bodyOf(c)["user"] ?? IDS.dana }),
    sample: { params: { login: "dana_okoye_work" }, body: { user: IDS.dana } },
  },
  noContent("/api/v1/admin/os-login-users/{login}"),
];

const SEVERITY_WRITES: readonly MockResponse[] = [
  { method: "PUT", path: "/api/v1/admin/severity-rules/{id}", body: (c: MockContext) => ({ id: c.params["id"] ?? "rule", ...RULE_BODY, ...bodyOf(c) }), sample: { params: { id: "security-high" }, body: RULE_BODY } },
  noContent("/api/v1/admin/severity-rules/{id}"),
];

/** The retention PUT: a dry run answers with the saved policy and the preview of the policy that was sent; a save answers with the policy. */
const RETENTION_PUT: MockResponse = {
  method: "PUT",
  path: "/api/v1/admin/retention",
  body: (c: MockContext) => {
    const input = bodyOf(c) as { keep_days?: number; keep_last_versions?: number };
    const days = input.keep_days ?? 180;
    const keep = input.keep_last_versions ?? 50;
    const preview = previewFor(days, keep);
    return c.query.get("dry_run") === "true" ? { ...retention(), dry_run_preview: preview } : { ...retention(), keep_days: days, keep_last_versions: keep, dry_run_preview: preview };
  },
  sample: { query: new URLSearchParams("dry_run=true"), body: { keep_days: 90, keep_last_versions: 50 } },
  samples: [{ query: new URLSearchParams(), body: { keep_days: 90, keep_last_versions: 50 } }],
};

export const scenarios: readonly Scenario[] = [
  admin("design-admin-t5-sentinels", "Sentinels (frames 1ao and 1bl): six NFS VMs, one of them (nfs-presitb-01) disconnected for 14 minutes with 38 records waiting, and one on an older version.", list("/api/v1/admin/sentinels", sentinels())),
  admin("design-admin-t5-sentinels-empty", "Sentinels with none registered (frame 1ap): edits on the VMs can't be attributed to a person.", list("/api/v1/admin/sentinels", [])),
  admin("design-admin-t5-oslogin", "OS Login users (frames 1at and 1av to 1ax): two manual mappings, a contractor and a second account, and the people they stand for.", list("/api/v1/admin/os-login-users", osLoginUsers()), ...OS_LOGIN_ANSWERS),
  admin("design-admin-t5-oslogin-empty", "OS Login users with no mapping (frame 1au): every name is matched by email where it can be.", list("/api/v1/admin/os-login-users", []), ...OS_LOGIN_ANSWERS),
  admin("design-admin-t5-severity", "Severity rules (frames 1ay and 1bm): four rules of the admin's own, one off and one with a change type that is not applied yet.", list("/api/v1/admin/severity-rules", severityRules()), ...SEVERITY_WRITES),
  admin("design-admin-t5-severity-empty", "Severity rules with none of the admin's own (frame 1az): only the built-in rules apply.", list("/api/v1/admin/severity-rules", []), ...SEVERITY_WRITES),
  admin("design-admin-t5-retention", "Retention (frames 1bd to 1bh): 180 days and the last 50 versions, changed by Priya Shah 40 days ago, with the dry run of the saved policy.", { method: "GET", path: "/api/v1/admin/retention", body: retention() }, list("/api/v1/admin/users", people()), RETENTION_PUT),
  admin(
    "design-admin-t5-retention-defaults",
    "Retention that was never changed (frame 1be): the defaults apply, and the hub has not computed a dry run yet.",
    { method: "GET", path: "/api/v1/admin/retention", body: retention({ updated_at: null, updated_by: null, dry_run_preview: null }) },
    list("/api/v1/admin/users", people()),
    RETENTION_PUT,
  ),
];
