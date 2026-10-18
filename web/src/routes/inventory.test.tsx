import { describe, expect, it } from "vitest";
import { createMemoryHistory } from "@tanstack/react-router";
import { ROUTES as HANDOFF } from "@/handoff/routes.generated";
import { createQueryClient } from "@/lib/query/client";
import { createAppRouter } from "@/router";
import { ROUTES } from "./table";

// The router is the handoff's screen inventory plus a COMMITTED list of approved additions, and nothing else. A route
// added without a person adding it here (and the plan's reviewers seeing the diff) fails this test. That is also what
// keeps "no AI chat" true: there is no route for one.
const APPROVED_ADDITIONS = [
  "/access-requested",
  "/access-disabled",
  "/access/disabled",
  "/me/github-token",
  "/me/tokens",
  "/swimlanes/:id/drift",
  "/swimlanes/:id/activity",
  "/docs/view",
  // The shapes of `LinkTemplates::default()` that are not a screen of their own (links_resolve.test.tsx): each only redirects.
  "/docs",
  "/grid",
  "/swimlanes/:id/restarts",
  "/swimlanes/:id/history",
  "/proposals/:id",
  "/drafts/:id",
];

describe("router inventory", () => {
  const router = createAppRouter({ queryClient: createQueryClient(), history: createMemoryHistory({ initialEntries: ["/"] }) });
  const routed = Object.keys(router.routesByPath)
    .map((p) => p.replace(/\$([A-Za-z]+)/g, ":$1"))
    .filter((p) => p !== "/")
    .sort();

  it("serves exactly the table's routes, plus the landing page", () => {
    expect(routed).toEqual(ROUTES.map((r) => r.path).sort());
    expect(Object.keys(router.routesByPath)).toContain("/");
  });

  it("is the handoff's routes plus the approved additions", () => {
    const handoff = new Set<string>(HANDOFF.rows.flatMap((r) => r.paths));
    const additions = routed.filter((p) => !handoff.has(p));
    expect(additions.sort()).toEqual([...APPROVED_ADDITIONS].sort());
  });

  it("has no route for a chat, an assistant or a prompt box", () => {
    for (const path of routed) expect(path).not.toMatch(/chat|assistant|copilot|ask|prompt|ai\b/i);
  });
});
