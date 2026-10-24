// A hub for the Admin journeys that remembers what it was told. The design's Admin scenarios answer every write with the same body
// whatever was sent, so a list does not change after a write; a journey that says "approve the request and the person appears in the
// table" needs the list to change. This is the design's data (src/mocks/design/admin/fixtures.ts) behind scenario responses that
// read and write a small state, in the test's own process. Each call makes a hub of its own, so no journey sees another's changes.
//
// It is a scenario like any other (src/mocks/scenario.ts): the harness serves it, and the page cannot tell it from the hub.
import type { AccessRequest, NotificationChannel, PathMappingRule, Role, User } from "../../src/api/generated/model/index.ts";
import { channels, IDS, NOW, pathRules, people, samRequest } from "../../src/mocks/design/admin/fixtures.ts";
import { problemBody } from "../../src/mocks/scenario.ts";
import { derive, type MockContext, type MockResponse } from "./scenarios";

export type AdminHub = {
  /** Serve this scenario. */
  readonly scenario: string;
  /** The same hub, except that the auto-notify setting is refused with 403 (it changes nothing). */
  readonly forbidding: string;
  readonly users: User[];
  readonly requests: AccessRequest[];
  readonly tenants: Record<string, string[]>;
  readonly autoNotify: Record<string, boolean>;
  readonly paths: PathMappingRule[];
  readonly channels: NotificationChannel[];
  /** Every webhook URL the hub was sent, by channel. A test looks here, and nowhere else, for what reached it. */
  readonly webhooks: Record<string, string>;
};

const idOf = (context: MockContext): string => context.params["id"] ?? "";
const bodyOf = (context: MockContext): Record<string, unknown> => (typeof context.body === "object" && context.body !== null ? (context.body as Record<string, unknown>) : {});
const page = <T>(items: readonly T[]): { items: readonly T[]; next: null } => ({ items, next: null });

export function adminHub(): AdminHub {
  const users = people();
  const requests: AccessRequest[] = [samRequest()];
  const tenants: Record<string, string[]> = { sit: ["sit1"], sitb: ["sit7"], sitc: ["sit9"], presita: ["sit11"], presitb: ["sit12"], presitc: ["sit13"] };
  const autoNotify: Record<string, boolean> = { sit: false, sitb: true };
  const paths = pathRules();
  const channelList = channels();
  const webhooks: Record<string, string> = {};

  const responses = (auto: "ok" | "forbidden"): MockResponse[] => [
    {
      method: "GET",
      path: "/api/v1/admin/users",
      body: (context: MockContext) => {
        const q = context.query.get("q")?.toLowerCase();
        const role = context.query.get("role");
        return page(users.filter((u) => (q === undefined || u.display_name.toLowerCase().includes(q) || u.email.toLowerCase().includes(q)) && (role === null || u.role === role)));
      },
    },
    {
      method: "PATCH",
      path: "/api/v1/admin/users/{id}",
      body: (context: MockContext) => {
        const user = users.find((u) => u.id === idOf(context));
        if (user === undefined) return problemBody(404, "not_found", "Not found");
        Object.assign(user, bodyOf(context));
        return user;
      },
      sample: { params: { id: IDS.lena }, body: { role: "admin" } },
    },
    {
      method: "GET",
      path: "/api/v1/admin/access-requests",
      body: (context: MockContext) => {
        const status = context.query.get("status");
        return page(requests.filter((r) => status === null || r.status === status));
      },
    },
    {
      method: "POST",
      path: "/api/v1/admin/access-requests/{id}/approve",
      body: (context: MockContext) => {
        const request = requests.find((r) => String(r.id) === idOf(context));
        if (request === undefined) return problemBody(404, "not_found", "Not found");
        request.status = "approved";
        request.decided_by = IDS.priya;
        request.decided_at = NOW;
        const added: User = { id: request.user, email: request.email, display_name: "Sam Okafor", role: (bodyOf(context)["role"] as Role | undefined) ?? request.requested_role, status: "active", requires_approval: false, github_login: null, last_seen_at: null };
        users.push(added);
        return added;
      },
      sample: { params: { id: "41" }, body: { role: "editor" } },
    },
    {
      method: "POST",
      path: "/api/v1/admin/access-requests/{id}/reject",
      body: (context: MockContext) => {
        const request = requests.find((r) => String(r.id) === idOf(context));
        if (request === undefined) return problemBody(404, "not_found", "Not found");
        request.status = "rejected";
        request.decided_by = IDS.priya;
        request.decided_at = NOW;
        return request;
      },
      sample: { params: { id: "41" } },
    },
    {
      method: "GET",
      path: "/api/v1/admin/swimlanes/{id}/tenants",
      body: (context: MockContext) => ({ tenants: tenants[idOf(context)] ?? [] }),
      sample: { params: { id: "sitb" } },
    },
    {
      method: "PUT",
      path: "/api/v1/admin/swimlanes/{id}/tenants",
      body: (context: MockContext) => {
        tenants[idOf(context)] = (bodyOf(context)["tenants"] as string[] | undefined) ?? [];
        return { tenants: tenants[idOf(context)] };
      },
      sample: { params: { id: "sitb" }, body: { tenants: ["sit7"] } },
    },
    {
      method: "GET",
      path: "/api/v1/admin/swimlanes/{id}/auto-notify",
      body: (context: MockContext) => ({ enabled: autoNotify[idOf(context)] ?? false }),
      sample: { params: { id: "sitb" } },
    },
    auto === "ok"
      ? {
          method: "PUT",
          path: "/api/v1/admin/swimlanes/{id}/auto-notify",
          body: (context: MockContext) => {
            autoNotify[idOf(context)] = bodyOf(context)["enabled"] === true;
            return { enabled: autoNotify[idOf(context)] };
          },
          sample: { params: { id: "sitb" }, body: { enabled: true } },
        }
      : { method: "PUT", path: "/api/v1/admin/swimlanes/{id}/auto-notify", status: 403, body: problemBody(403, "forbidden", "Forbidden") },
    { method: "GET", path: "/api/v1/admin/path-mappings", body: () => page(paths) },
    {
      method: "PUT",
      path: "/api/v1/admin/path-mappings/{id}",
      body: (context: MockContext) => {
        const rule = { id: idOf(context), ...bodyOf(context) } as PathMappingRule;
        const at = paths.findIndex((p) => p.id === rule.id);
        if (at >= 0) paths[at] = rule;
        else paths.push(rule);
        return rule;
      },
      sample: { params: { id: "rule" }, body: { base_root: "config", tenant_root: "data/config", swimlanes: [] } },
    },
    {
      method: "DELETE",
      path: "/api/v1/admin/path-mappings/{id}",
      status: 204,
      body: (context: MockContext) => {
        const at = paths.findIndex((p) => p.id === idOf(context));
        if (at >= 0) paths.splice(at, 1);
        return undefined;
      },
      sample: { params: { id: "default" } },
    },
    { method: "GET", path: "/api/v1/admin/notifications", body: () => page(channelList) },
    {
      method: "PUT",
      path: "/api/v1/admin/notifications/{id}",
      body: (context: MockContext) => {
        const { webhook_url: url, ...rest } = bodyOf(context);
        const id = idOf(context);
        if (typeof url === "string") webhooks[id] = url;
        const existing = channelList.find((c) => c.id === id);
        const channel = { id, ...rest, webhook_configured: typeof url === "string" || existing?.webhook_configured === true } as NotificationChannel;
        const at = channelList.findIndex((c) => c.id === id);
        if (at >= 0) channelList[at] = channel;
        else channelList.push(channel);
        return channel;
      },
      sample: { params: { id: "approvals" }, body: { name: "Approvals", min_severity: "low", kinds: ["proposal_pending"], enabled: true } },
    },
  ];

  return {
    scenario: derive("design-admin-swimlanes", responses("ok"), "journey-admin-hub"),
    forbidding: derive("design-admin-swimlanes", responses("forbidden"), "journey-admin-hub-forbidding"),
    users,
    requests,
    tenants,
    autoNotify,
    paths,
    channels: channelList,
    webhooks,
  };
}
