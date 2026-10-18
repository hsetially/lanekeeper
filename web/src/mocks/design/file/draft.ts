// The draft page's scenarios (CG17): a draft a tool saved for Priya Shah to read and apply. Loaded by Node as well as by Vite.
import type { Draft } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockContext, type Scenario } from "../../scenario.ts";
import { SHELL_LANES } from "../shell/index.ts";
import { coreComparison, CORE_PATH, H_NEWER, H_NFS, LANE, NOW, PRIYA } from "./data.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

function draft(status: Draft["status"]): Draft {
  const diff = coreComparison("nfs:sitb", "nfs:sitb");
  return {
    id: "7",
    author: PRIYA,
    swimlane: LANE,
    path: CORE_PATH,
    expected: { state: "hash", hash: H_NFS },
    new_hash: H_NEWER,
    hunks: diff.hunks,
    setting_changes: diff.setting_changes,
    status,
    expires_at: NOW + 24 * 3_600_000,
    severity: "medium",
    effects: [
      [{ namespace: "csp", name: "tx-infinity-core" }, "needs_notify_or_restart"],
      [{ namespace: "csp", name: "tx-infinity-gateway" }, "live_within_ttl"],
    ],
  };
}

function draftScenario(id: string, description: string, status: Draft["status"], apply: { status?: number; body: unknown }): Scenario {
  return {
    id,
    description,
    responses: [
      { method: "GET", path: "/api/v1/me", body: meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" }), headers: { "set-cookie": csrfCookie } },
      { method: "POST", path: "/auth/logout", status: 204 },
      { method: "GET", path: "/api/v1/swimlanes", body: { items: SHELL_LANES, next: null } },
      { method: "GET", path: "/api/v1/swimlanes/{id}", sample: { params: { id: LANE } }, body: ({ params }: MockContext) => ({ ...(SHELL_LANES.find((l) => l.id === (params["id"] ?? LANE)) ?? SHELL_LANES[1]), drift_counts: [["nfs_ahead", 4]], services: [], auto_notify: false }) },
      { method: "GET", path: "/api/v1/proposals", body: { items: [], next: null } },
      { method: "GET", path: "/api/v1/admin/access-requests", body: { items: [], next: null } },
      { method: "GET", path: "/api/v1/drafts/{id}", sample: { params: { id: "7" } }, headers: { "cache-control": "no-store" }, body: draft(status) },
      { method: "POST", path: "/api/v1/drafts/{id}/apply", sample: { params: { id: "7" } }, ...apply },
    ],
  };
}

export const draftScenarios: readonly Scenario[] = [
  draftScenario("design-file-draft", "Draft: a saved change to the core file, ready to review and apply; applying it succeeds.", "draft", { body: { outcome: "applied", hash: H_NEWER, audit_id: 43 } }),
  draftScenario("design-file-draft-conflict", "Draft, 409: the file changed since the draft was made; nothing is applied.", "draft", { status: 409, body: problemBody(409, "conflict", "Conflict", { current: H_NEWER }) }),
  draftScenario("design-file-draft-applied", "Draft that was already applied: the page says so and Apply is off.", "applied", { body: { outcome: "applied", hash: H_NEWER, audit_id: 43 } }),
];
