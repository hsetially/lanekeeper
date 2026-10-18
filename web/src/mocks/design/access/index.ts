// The design-parity scenarios of slice S1 (sign-in, access pending and disabled, onboarding, My GitHub token, personal MCP tokens): the
// data of the Access Screen of the handoff (`Lanekeeper Access Screen.dc.html` and the board `Lanekeeper Access and Onboarding`).
//
//  - Marco Diaz is an Editor whose GitHub login is `mdiaz`; the onboarding shows him with no token stored, and "My GitHub token" with a
//    token that ends in 5 days (frame 1s).
//  - The onboarding's states are decided by what the hub answers to the token, so each state is a scenario of its own: the one that
//    accepts the token (the "empty" frame and, after a paste, the success), and one for each refusal the design draws (classic,
//    missing repository access, login mismatch). "Verifying" is a request that has not been answered yet: a scenario is data and
//    cannot hold a response, so the visual run holds the PUT itself.
//  - The pages before sign-in (sign-in, access requested, access disabled) ask the hub for nothing; their scenario answers only the
//    sign-out.
// Times are derived from the frozen clock the visual runs use (e2e/visual/render-references.mjs: 1_760_000_000_000), so "in 5 days" is
// the same on every run.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { AccessRequest, GithubCredential, McpToken, Me, Role } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockResponse, type Scenario } from "../../scenario.ts";
import { MARCO, SHELL_LANES } from "../shell/index.ts";

const NOW = 1_760_000_000_000;
const DAY = 86_400_000;
const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";
const SCOPES = ["push:acme/configuration-base-saas", "push:acme/csp-tenant-data"];

const credential = (over: Partial<GithubCredential> = {}): GithubCredential => ({ github_login: "mdiaz", verified_at: NOW, scopes: SCOPES, expires_at: NOW + 90 * DAY, expires_soon: false, ...over });

/** What the shell asks of every signed-in screen, and who is signed in. */
function session(role: Role, over: Partial<Me> = {}): MockResponse[] {
  return [
    { method: "GET", path: "/api/v1/me", body: meBody(role, { id: MARCO, display_name: "Marco Diaz", email: "marco.diaz@example.test", github_login: "mdiaz", ...over }), headers: { "set-cookie": csrfCookie } },
    { method: "POST", path: "/auth/logout", status: 204 },
    { method: "GET", path: "/api/v1/swimlanes", body: { items: SHELL_LANES, next: null } },
  ];
}

const refusal = (status: number, code: string, title: string, extra: Record<string, unknown> = {}): MockResponse => ({ method: "PUT", path: "/api/v1/me/github-token", status, body: problemBody(status, code, title, extra) });
const accepted: MockResponse = { method: "PUT", path: "/api/v1/me/github-token", body: credential() };
const noToken: MockResponse = { method: "GET", path: "/api/v1/me/github-token", status: 404, body: problemBody(404, "not_found", "Not found") };

const onboarding = (id: string, description: string, put: MockResponse): Scenario => ({
  id,
  description,
  responses: [...session("editor", { needs_github_token: true }), noToken, put],
});

const accessRequest: AccessRequest = { id: 31, user: MARCO, email: "marco.diaz@example.test", requested_role: "editor", reason: "To fix a typo in the sit7 settings", status: "pending", created_at: NOW, decided_by: null, decided_at: null };

const mcpToken = (id: number, label: string, over: Partial<McpToken> = {}): McpToken => ({ id, label, created_at: NOW - 2 * DAY, expires_at: NOW + 5 * DAY, revoked_at: null, last_used_at: NOW - 3_600_000, ...over });

export const scenarios: readonly Scenario[] = [
  {
    id: "design-access-public",
    description: "Sign in, access requested and access disabled (frames 1a to 1f): a public route asks the hub for nothing. Only sign-out answers.",
    responses: [{ method: "POST", path: "/auth/logout", status: 204 }],
  },
  onboarding("design-access-onboarding", "GitHub token onboarding (frames 1g, 1h, 1k, 1l): Marco Diaz, an Editor with no token stored. The hub accepts the token that is pasted: login mdiaz, ending in 90 days.", accepted),
  onboarding(
    "design-access-onboarding-classic",
    "Onboarding, a classic token (frames 1m, 1n): the hub refuses it by policy.",
    refusal(422, "classic_token", "Classic tokens are not accepted"),
  ),
  onboarding(
    "design-access-onboarding-repo",
    "Onboarding, missing repository access (frames 1o, 1p): the token can push to the base repository and not to the tenant repository.",
    refusal(422, "token_missing_access", "The token cannot push to both repositories", { base: true, tenant: false }),
  ),
  onboarding(
    "design-access-onboarding-mismatch",
    "Onboarding, login mismatch (frames 1q, 1r): the token belongs to another GitHub account than the one linked to Marco (mdiaz).",
    refusal(422, "github_account_mismatch", "The token belongs to another GitHub account"),
  ),
  {
    id: "design-access-mytoken",
    description: "My GitHub token (frames 1s, 1t): Marco Diaz, a token for mdiaz that ends in 5 days, with the expiry banner and badge.",
    responses: [...session("editor"), { method: "GET", path: "/api/v1/me/github-token", body: credential({ verified_at: NOW - 14 * 60_000, expires_at: NOW + 5 * DAY, expires_soon: true }) }],
  },
  {
    id: "design-access-mytoken-ok",
    description: "My GitHub token with a token that is not near its end: no banner, no badge.",
    responses: [...session("editor"), { method: "GET", path: "/api/v1/me/github-token", body: credential({ verified_at: NOW - 14 * 60_000, expires_at: NOW + 60 * DAY }) }],
  },
  {
    id: "design-access-mytoken-expired",
    description: "My GitHub token whose token has ended yesterday: the danger banner says Save and Raise PR do not work.",
    responses: [...session("editor"), { method: "GET", path: "/api/v1/me/github-token", body: credential({ verified_at: NOW - 40 * DAY, expires_at: NOW - DAY, expires_soon: true }) }],
  },
  {
    id: "design-access-mytoken-none",
    description: "My GitHub token with no token stored (the hub answers 404): the empty state and the form to add one.",
    responses: [...session("editor", { needs_github_token: true }), noToken, { method: "PUT", path: "/api/v1/me/github-token", body: credential() }],
  },
  {
    id: "design-access-mcp-tokens",
    description: "Personal MCP tokens (no handoff frame): a Viewer with a token that works, one that ended and one that was revoked.",
    responses: [
      ...session("viewer"),
      { method: "GET", path: "/api/v1/me/mcp-tokens", body: { items: [mcpToken(3, "laptop"), mcpToken(2, "old", { created_at: NOW - 20 * DAY, expires_at: NOW - 13 * DAY, last_used_at: null }), mcpToken(1, "gone", { revoked_at: NOW - DAY, last_used_at: null })], next: null } },
    ],
  },
  {
    id: "design-access-mcp-token-issued",
    description: "Making a personal MCP token (no handoff frame): the hub answers the POST with the new token, which it shows once. The copy button's end-to-end test uses it.",
    responses: [
      ...session("viewer"),
      { method: "GET", path: "/api/v1/me/mcp-tokens", body: { items: [mcpToken(3, "laptop")], next: null } },
      { method: "POST", path: "/api/v1/me/mcp-tokens", status: 201, body: { ...mcpToken(4, "ci", { last_used_at: null }), token: "lkpat_ZxY9w8V7u6T5s4R3q2P1o0NmLkJiHgFeDcBaZxY9w8V7u6T5s4R3q2P1o0N" } },
    ],
  },
  {
    id: "design-access-mcp-off",
    description: "Personal MCP tokens on a hub where the mode is not switched on (the list answers 404): the empty state.",
    responses: [...session("viewer"), { method: "GET", path: "/api/v1/me/mcp-tokens", status: 404, body: problemBody(404, "not_found", "Not found") }],
  },
  {
    id: "design-access-viewer",
    description: "A Viewer who can ask for a different role (the permission state's Request a role change): the hub accepts the request.",
    responses: [...session("viewer"), { method: "POST", path: "/api/v1/me/access-request", status: 202, body: accessRequest }],
  },
  {
    id: "design-access-viewer-open-request",
    description: "A Viewer who asks for a role while an earlier request is still waiting: the hub answers 409.",
    responses: [...session("viewer"), { method: "POST", path: "/api/v1/me/access-request", status: 409, body: problemBody(409, "conflict", "The request conflicts with the current state") }],
  },
];
