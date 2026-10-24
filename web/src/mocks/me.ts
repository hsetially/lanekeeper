// The signed-in user of the mock scenarios. Loaded by Node (the e2e harness) as well as by Vite: relative imports with the
// `.ts` extension and `import type` for types.
import type { Me, Role } from "../api/generated/model/index.ts";

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
