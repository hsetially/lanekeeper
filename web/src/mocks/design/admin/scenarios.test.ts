import { describe, expect, it } from "vitest";
import { scenarios } from "./index.ts";

// The design-parity scenarios of the Admin area (the contract check is src/mocks/mocks.test.ts, which validates every response).

describe("the Admin scenarios", () => {
  it("are named for the slice and for the section they draw", () => {
    const ids = scenarios.map((s) => s.id);
    expect(ids.every((id) => id.startsWith("design-admin-"))).toBe(true);
    for (const section of ["users", "swimlanes", "paths", "merge", "tiers", "folders", "notify", "mcp"]) expect(ids, section).toContain(`design-admin-${section}`);
    for (const state of ["users-empty", "users-conflict", "swimlanes-empty", "paths-empty", "merge-empty", "tiers-empty", "folders-empty", "notify-empty", "mcp-empty", "mcp-off", "viewer"]) expect(ids, state).toContain(`design-admin-${state}`);
  });

  it("describe what they draw", () => {
    for (const scenario of scenarios) expect(scenario.description.length, scenario.id).toBeGreaterThan(30);
  });

  it("sign in an Admin and answer the pending requests the navigation counts, except the Viewer's", () => {
    for (const scenario of scenarios.filter((s) => s.id !== "design-admin-viewer")) {
      const me = scenario.responses.find((r) => r.path === "/api/v1/me")?.body as { role?: string } | undefined;
      expect(me?.role, scenario.id).toBe("admin");
      expect(scenario.responses.some((r) => r.method === "GET" && r.path === "/api/v1/admin/access-requests"), scenario.id).toBe(true);
    }
  });

  it("answer the pending requests with the page's own, not the Shell's empty list (the harness uses the first answer that matches)", () => {
    for (const id of ["design-admin-users", "design-admin-swimlanes", "design-admin-paths"]) {
      const first = scenarios.find((s) => s.id === id)?.responses.find((r) => r.method === "GET" && r.path === "/api/v1/admin/access-requests");
      expect((first?.body as { items?: unknown[] } | undefined)?.items, id).toHaveLength(1);
    }
    const empty = scenarios.find((s) => s.id === "design-admin-users-empty")?.responses.find((r) => r.method === "GET" && r.path === "/api/v1/admin/access-requests");
    expect((empty?.body as { items?: unknown[] } | undefined)?.items).toHaveLength(0);
  });

  it("give the Viewer nothing of the Admin area, so the permission state is all there is", () => {
    const viewer = scenarios.find((s) => s.id === "design-admin-viewer");
    expect((viewer?.responses.find((r) => r.path === "/api/v1/me")?.body as { role?: string } | undefined)?.role).toBe("viewer");
    expect(viewer?.responses.some((r) => r.path.startsWith("/api/v1/admin"))).toBe(false);
  });

  it("never carry a secret the hub would not send: no webhook address in any list", () => {
    expect(JSON.stringify(scenarios.map((s) => s.responses.map((r) => (typeof r.body === "function" ? null : r.body))))).not.toMatch(/webhook\.office\.com|webhook_url/);
  });
});
