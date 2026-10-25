// The design-parity scenarios of task T5, slice A (attribution and severity): the places that say who changed a file, how
// Lanekeeper knows, and how sure it is, on data in which every source of D73 and every level of confidence appears. They reuse the
// answers of the other slices' scenarios (the File page's, the Shell's) and replace only the history and the audit read, so the
// screens around the chip are the ones the design draws.
//
//   design-attribution-history   The File page with a History panel of nine versions: every source, every severity, a user the
//                                Admin's list knows and one it does not; the head of the file says who changed it last.
//   design-attribution-hostile   The same page, where the OS logins, the job name and the executables are hostile text.
//   design-attribution-audit     The Audit log (and a swimlane's Activity tab) with an event for every source and level.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { Attribution, Version } from "../../../api/generated/model/index.ts";
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";
import { meBody } from "../../me.ts";
import { fileScenario } from "../file/build.ts";
import { coreFile } from "../file/data.ts";
import { scenarios as shellScenarios } from "../shell/index.ts";
import { scenarios as swimlaneScenarios } from "../swimlanes/index.ts";
import { auditEvents, BY_SOURCE, historyVersions, hostileVersions, PRIYA, USERS } from "./data.ts";

const FILES = "/api/v1/swimlanes/{id}/files";
const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";
const query = (text: string): Partial<MockContext> => ({ query: new URLSearchParams(text) });

const users: MockResponse = { method: "GET", path: "/api/v1/admin/users", body: { items: USERS, next: null }, sample: query("limit=500") };

/** The File page with the history, and the head of the file, that this slice chooses. */
function history(id: string, description: string, versions: readonly Version[], headOfFile: Attribution): Scenario {
  const core = coreFile({ masked: false });
  const base = fileScenario({ id, description, file: { ...core, entry: { ...core.entry, attribution: headOfFile } } });
  return {
    id,
    description,
    responses: [
      { method: "GET", path: `${FILES}/history`, sample: { params: { id: "sitb" } }, body: { items: versions, next: null } },
      ...base.responses.filter((r) => !(r.method === "GET" && r.path === `${FILES}/history`)),
      users,
    ],
  };
}

function auditLog(): Scenario {
  // A swimlane's Activity tab reads the swimlane and its tree before it reads the audit log: the Swimlanes slice's answers for them.
  const lane = (swimlaneScenarios.find((s) => s.id === "design-swimlanes-six")?.responses ?? []).filter((r) => r.method === "GET" && (r.path === "/api/v1/swimlanes/{id}" || r.path === "/api/v1/swimlanes/{id}/tree"));
  const shell = (shellScenarios.find((s) => s.id === "design-shell-admin")?.responses ?? []).filter((r) => r.path !== "/api/v1/me" && r.path !== "/api/v1/admin/users");
  const read: MockResponse = {
    method: "GET",
    path: "/api/v1/admin/audit",
    sample: query("limit=100"),
    samples: [query("swimlane=sitb&limit=100")],
    body: ({ query: q }: MockContext) => ({ items: auditEvents().filter((e) => q.get("swimlane") === null || e.swimlane === q.get("swimlane")), next: null }),
  };
  const me: MockResponse = { method: "GET", path: "/api/v1/me", body: meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" }), headers: { "set-cookie": csrfCookie } };
  return { id: "design-attribution-audit", description: "The Audit log with an event for every source of attribution and every level of confidence, for Priya Shah, an Admin whose user list names Priya and Marco and not the user who was removed.", responses: [me, ...shell, ...lane, users, read] };
}

export const scenarios: readonly Scenario[] = [
  history(
    "design-attribution-history",
    "The File page for Priya Shah, an Admin, with a History panel of nine versions: every source of attribution, every level of confidence and every severity; the file's head says an OS login changed it last.",
    historyVersions(),
    BY_SOURCE.login,
  ),
  history(
    "design-attribution-hostile",
    "The File page with hostile text in the attribution: a right-to-left override and a zero-width space in OS logins, a job name and executables, markup as a login, and a login of 256 characters.",
    hostileVersions(),
    BY_SOURCE.unknown,
  ),
  auditLog(),
];
