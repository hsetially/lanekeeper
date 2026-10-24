// The design-parity scenarios of the pull-request pieces (T5, slice B): the Pull requests page of a swimlane (the page the MCP
// links and "Find my pull request" lead to; the handoff has no frame for it, so these are app-owned baselines), and the Drift tab
// with the PR badges of Consequence dialogs, frame 1l. The PR dialog and its job (frames 1a to 1k) are the file view's scenarios
// (`design-file-pr*`). The signed-in person is Priya Shah, an Admin; the page is open to a Viewer too, and does not differ.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type` for every type.
import type { PrLink } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";
import { scenarios as shellScenarios, PRIYA } from "../shell/index.ts";
import { scenarios as swimlaneScenarios } from "../swimlanes/index.ts";
import { HOSTILE, LINKS } from "./fixtures.ts";

const PRS = "/api/v1/prs";
const query = (text: string): Partial<MockContext> => ({ query: new URLSearchParams(text) });

/** The hub's list: by swimlane and by state when they are asked, newest (highest id) first, one page. */
function list(links: readonly PrLink[]): MockResponse {
  return {
    method: "GET",
    path: PRS,
    body: ({ query: q }: MockContext) => ({
      items: links.filter((l) => (q.get("swimlane") === null || l.swimlane === q.get("swimlane")) && (q.get("state") === null || l.state === q.get("state"))).sort((a, b) => b.id - a.id),
      next: null,
    }),
    sample: query("swimlane=sitb&state=open&limit=100"),
    samples: [query("swimlane=sitb&limit=100"), query("swimlane=sitb&state=merged&limit=100"), query("swimlane=sitb&state=closed&limit=100")],
  };
}

/** The shell around the page: who is signed in, the swimlanes, the sidebar's counts. */
function chrome(): readonly MockResponse[] {
  const me: MockResponse = {
    method: "GET",
    path: "/api/v1/me",
    body: meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" }),
    headers: { "set-cookie": "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax" },
  };
  const rest = shellScenarios.find((s) => s.id === "design-shell-admin")?.responses.filter((r) => r.path !== "/api/v1/me" && r.path !== PRS) ?? [];
  return [me, ...rest];
}

/** Frame 1l: the Drift tab of sitb, decorated. Only the links of the frame: #418 open, #417 merged, and the one of another swimlane. */
function driftBadges(): Scenario {
  const base = swimlaneScenarios.find((s) => s.id === "design-swimlanes-six")?.responses.filter((r) => r.path !== PRS) ?? [];
  return {
    id: "design-prs-drift-badges",
    description: "The Drift tab of sitb with the PR badges of frame 1l: PR #418 open on the tenant file, Fix merged, awaiting sync on application.properties, and no badge on the others.",
    responses: [list(LINKS.filter((l) => l.pr_number === 418 || l.pr_number === 417 || l.pr_number === 409)), ...base],
  };
}

export const scenarios: readonly Scenario[] = [
  { id: "design-prs-list", description: "The Pull requests page of sitb: two open (one of them carrying two files), a merged one and a closed one.", responses: [list(LINKS), ...chrome()] },
  { id: "design-prs-empty", description: "The Pull requests page of sitb with no pull request in any state.", responses: [list([]), ...chrome()] },
  { id: "design-prs-hostile", description: "The Pull requests page with links the page must not trust: a hidden control character in a path, an address that is a script, one with credentials, one that is http, and a 600-character path.", responses: [list(HOSTILE), ...chrome()] },
  driftBadges(),
];
