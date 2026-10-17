// The scenarios of T1: who is signed in. T4's slices add their own beside this file (src/mocks/design/<slice>/) for the
// design-parity data of each screen.
import type { Me, Role } from "../api/generated/model/index.ts";
import { problemBody, type Scenario } from "./scenario.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

export function meBody(role: Role | null, overrides: Partial<Me> = {}): Me {
  return {
    id: "11111111-1111-4111-8111-111111111111:22222222-2222-4222-8222-222222222222",
    email: "ada.lovelace@example.test",
    display_name: "Ada Lovelace",
    role,
    status: "active",
    requires_approval: false,
    github_login: null,
    last_seen_at: null,
    swimlane_access: "all",
    needs_github_token: false,
    ...overrides,
  };
}

const signedIn = (id: string, description: string, role: Role | null, overrides: Partial<Me> = {}): Scenario => ({
  id,
  description,
  responses: [
    { method: "GET", path: "/api/v1/me", body: meBody(role, overrides), headers: { "set-cookie": csrfCookie } },
    { method: "POST", path: "/auth/logout", status: 204 },
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

export const DEFAULT_SCENARIO = "viewer";
