// The scenarios of T1: who is signed in. T4's slices add their own beside this file (src/mocks/design/<slice>/) for the
// design-parity data of each screen.
import type { Me, Role } from "../api/generated/model/index.ts";
import { designScenarios } from "./design/index.ts";
import { liveScenarios } from "./live.ts";
import { meBody } from "./me.ts";
import { perfScenarios } from "./perf/scenarios.ts";
import { problemBody, type Scenario } from "./scenario.ts";

export { meBody };

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

const signedIn = (id: string, description: string, role: Role | null, overrides: Partial<Me> = {}): Scenario => ({
  id,
  description,
  responses: [
    { method: "GET", path: "/api/v1/me", body: meBody(role, overrides), headers: { "set-cookie": csrfCookie } },
    { method: "POST", path: "/auth/logout", status: 204 },
    // What the app shell asks of every signed-in screen: the swimlane list (the sidebar's findings count and the switcher), and
    // for an Admin the pending proposals and the access requests that wait (the sidebar's Approvals and Admin counts). Empty, so a
    // screen's own scenario is the one with data.
    { method: "GET", path: "/api/v1/swimlanes", body: { items: [], next: null } },
    ...(role === "admin"
      ? [
          { method: "GET" as const, path: "/api/v1/proposals", body: { items: [], next: null } },
          { method: "GET" as const, path: "/api/v1/admin/access-requests", body: { items: [], next: null } },
        ]
      : []),
  ],
});

export const scenarios: Record<string, Scenario> = {
  "signed-out": {
    id: "signed-out",
    description: "No session: every call is a 401.",
    responses: [
      { method: "GET", path: "/api/v1/me", status: 401, body: problemBody(401, "unauthenticated", "Sign in required") },
      { method: "POST", path: "/auth/logout", status: 204 },
    ],
  },
  viewer: signedIn("viewer", "A signed-in Viewer.", "viewer"),
  editor: signedIn("editor", "A signed-in Editor.", "editor"),
  operator: signedIn("operator", "A signed-in Operator.", "operator"),
  admin: signedIn("admin", "A signed-in Admin.", "admin"),
  "needs-token": signedIn("needs-token", "An Editor whose GitHub token is missing.", "editor", { needs_github_token: true }),
  pending: {
    id: "pending",
    description: "A user who has no role yet: the hub answers 403 access-pending.",
    responses: [{ method: "GET", path: "/api/v1/me", status: 403, body: problemBody(403, "access-pending", "Access pending") }],
  },
  disabled: {
    id: "disabled",
    description: "A disabled user: the hub answers 403 access-disabled.",
    responses: [{ method: "GET", path: "/api/v1/me", status: 403, body: problemBody(403, "access-disabled", "Access disabled") }],
  },
};

// The data of the performance runs (src/mocks/perf): an Editor, a 5,000-row grid, a 6,000-line diff, a list of swimlanes.
Object.assign(scenarios, perfScenarios);
// The data of the live-update runs (src/mocks/live.ts): a list whose first row counts how often it was asked for.
Object.assign(scenarios, liveScenarios);

// The design-parity data of each T4 slice (src/mocks/design/<slice>/): the pages' own sample data, for the visual comparison.
Object.assign(scenarios, designScenarios);

export const DEFAULT_SCENARIO = "viewer";
