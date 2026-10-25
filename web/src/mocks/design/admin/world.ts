// A small hub for the tests of the Admin screens: a world (who is signed in, the people, the requests, the rules, the agents) and the
// function that answers `fetch` for it. A write changes the world the way the hub would, so the list that is asked for again after
// the write shows the new state; a test can also make a call fail, or hold it, to play the states the hub gives in bad moments.
// Test code only: nothing outside the tests imports it.
import type {
  AccessRequest,
  AgentRegistration,
  FolderMapping,
  HostTier,
  McpToken,
  MergeModeRule,
  NotificationChannel,
  PathMappingRule,
  Role,
  SwimlaneSummary,
  User,
} from "../../../api/generated/model/index.ts";
import { json, me, problem, type Api, type Call } from "../../../test/app.tsx";
import { channels, folders, hostTiers, IDS, mergeRules, pathRules, people, registrations, samRequest, NOW } from "./fixtures.ts";
import { SHELL_LANES } from "../shell/index.ts";

/** A call the test wants to answer itself: the first one whose `match` is true for a call is used; `once` drops it after use. */
export type Override = { match: (call: Call) => boolean; respond: (call: Call) => Response | Promise<Response>; once?: boolean };

export type World = {
  role: Role | null;
  /** The signed-in person; the Users table marks this row "You". */
  meId: string;
  users: User[];
  requests: AccessRequest[];
  lanes: SwimlaneSummary[];
  agents: AgentRegistration[];
  tenants: Record<string, string[]>;
  autoNotify: Record<string, boolean>;
  paths: PathMappingRule[];
  merge: MergeModeRule[];
  tiers: HostTier[];
  folders: FolderMapping[];
  channels: NotificationChannel[];
  /** The signed-in person's own MCP tokens; `null` is the hub with personal tokens switched off (the list answers 404). */
  mcp: McpToken[] | null;
  /** What the hub answers to a join token request, and to a new MCP token. The tests look for these strings in every cache. */
  joinToken: string;
  mcpSecret: string;
  overrides: Override[];
  /** Paths (pathname only) whose answer never arrives: the loading state. */
  hold: Set<string>;
  /** Every webhook URL the hub was sent, by channel id. A test reads it to see what reached the server. */
  webhooks: Record<string, string>;
};

export function newWorld(role: Role | null = "admin"): World {
  return {
    role,
    meId: IDS.priya,
    users: people(),
    requests: [samRequest()],
    lanes: [...SHELL_LANES],
    agents: registrations(),
    tenants: Object.fromEntries(SHELL_LANES.map((l) => [l.id, [...l.tenants]])),
    autoNotify: { sit: false, sitb: true },
    paths: pathRules(),
    merge: mergeRules(),
    tiers: hostTiers(),
    folders: folders(),
    channels: channels(),
    mcp: [],
    joinToken: "lkjt_7f3a9c21e8b04d5a96c1f2e07b38d4a5",
    mcpSecret: "lkpat_ZxY9w8V7u6T5s4R3q2P1o0NmLkJiHgFeDcBaZxY9w8V7u6T5s4R3q2P1o0N",
    overrides: [],
    hold: new Set(),
    webhooks: {},
  };
}

const page = (items: readonly unknown[]): Response => json({ items, next: null });
const noContent = (): Response => new Response(null, { status: 204 });
const notFound = (): Response => problem(404, "not_found");
const body = (call: Call): Record<string, unknown> => {
  try {
    const parsed: unknown = call.body === undefined ? {} : JSON.parse(call.body);
    return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
};

const text = (value: unknown): string => (typeof value === "string" ? value : "");

/** Replace the entry with this id, or add one (what a PUT of a rule does). */
function upsert<T extends { id: string }>(list: T[], item: T): T[] {
  return list.some((x) => x.id === item.id) ? list.map((x) => (x.id === item.id ? item : x)) : [...list, item];
}

const activeAdmins = (users: readonly User[]): number => users.filter((u) => u.role === "admin" && u.status === "active").length;

export function worldApi(world: World): Api {
  return (call) => {
    const url = new URL(call.path, "http://hub.test");
    const path = url.pathname;
    for (const [index, override] of world.overrides.entries()) {
      if (override.match(call)) {
        if (override.once === true) world.overrides.splice(index, 1);
        return override.respond(call);
      }
    }
    if (world.hold.has(path)) return new Promise<Response>(() => undefined);

    if (path === "/api/v1/me") return json(me(world.role, { id: world.meId }));
    if (path === "/api/v1/swimlanes") return page(world.lanes);
    if (path === "/api/v1/proposals") return page([]);

    // Users and access requests.
    if (path === "/api/v1/admin/users" && call.method === "GET") {
      const q = url.searchParams.get("q")?.toLowerCase();
      const status = url.searchParams.get("status");
      return page(world.users.filter((u) => (q === undefined || u.display_name.toLowerCase().includes(q) || u.email.toLowerCase().includes(q)) && (status === null || u.status === status)));
    }
    const userId = /^\/api\/v1\/admin\/users\/([^/]+)$/.exec(path)?.[1];
    if (userId !== undefined && call.method === "PATCH") {
      const id = decodeURIComponent(userId);
      const user = world.users.find((u) => u.id === id);
      if (user === undefined) return notFound();
      const change = body(call) as Partial<Pick<User, "role" | "status" | "requires_approval">>;
      const next: User = { ...user, ...change };
      if (id === world.meId && change.role !== undefined && change.role !== user.role) return problem(409, "conflict");
      const after = world.users.map((u) => (u.id === id ? next : u));
      if (activeAdmins(after) < 1) return problem(409, "conflict");
      world.users = after;
      return json(next);
    }
    if (path === "/api/v1/admin/access-requests" && call.method === "GET") {
      const status = url.searchParams.get("status");
      return page(world.requests.filter((r) => status === null || r.status === status));
    }
    const decision = /^\/api\/v1\/admin\/access-requests\/(\d+)\/(approve|reject)$/.exec(path);
    if (decision && call.method === "POST") {
      const request = world.requests.find((r) => String(r.id) === decision[1]);
      if (request === undefined) return notFound();
      if (request.status !== "pending") return problem(409, "conflict");
      const approve = decision[2] === "approve";
      const decided: AccessRequest = { ...request, status: approve ? "approved" : "rejected", decided_by: world.meId, decided_at: NOW };
      world.requests = world.requests.map((r) => (r.id === request.id ? decided : r));
      if (!approve) return json(decided);
      const role = body(call)["role"] as Role;
      const added: User = { id: request.user, email: request.email, display_name: "Sam Okafor", role, status: "active", requires_approval: false, github_login: null, last_seen_at: null };
      world.users = [...world.users, added];
      return json(added);
    }

    // Swimlanes and agents.
    if (path === "/api/v1/admin/agents" && call.method === "GET") return page(world.agents);
    const agent = /^\/api\/v1\/admin\/agents\/([^/]+)(\/join-token)?$/.exec(path);
    if (agent) {
      const swimlane = decodeURIComponent(agent[1] ?? "");
      if (!world.lanes.some((l) => l.id === swimlane)) return notFound();
      if (agent[2] !== undefined) return json({ token: world.joinToken, swimlane, expires_at: NOW + 24 * 3_600_000 }, 201);
      if (call.method === "PUT") {
        const email = text(body(call)["service_account_email"]);
        const existing = world.agents.find((a) => a.swimlane === swimlane);
        const next: AgentRegistration = existing ? { ...existing, service_account_email: email } : { swimlane, service_account_email: email, status: "never_seen", connected_replica: null, last_seen_at: null, certificate_expires_at: null };
        world.agents = existing ? world.agents.map((a) => (a.swimlane === swimlane ? next : a)) : [...world.agents, next];
        return json(next);
      }
      if (call.method === "DELETE") {
        world.agents = world.agents.filter((a) => a.swimlane !== swimlane);
        return noContent();
      }
    }
    const setting = /^\/api\/v1\/admin\/swimlanes\/([^/]+)\/(tenants|auto-notify)$/.exec(path);
    if (setting) {
      const swimlane = decodeURIComponent(setting[1] ?? "");
      if (!world.lanes.some((l) => l.id === swimlane)) return notFound();
      if (setting[2] === "tenants") {
        if (call.method === "PUT") world.tenants[swimlane] = (body(call)["tenants"] as string[] | undefined) ?? [];
        return json({ tenants: world.tenants[swimlane] ?? [] });
      }
      if (call.method === "PUT") world.autoNotify[swimlane] = body(call)["enabled"] === true;
      return json({ enabled: world.autoNotify[swimlane] ?? false });
    }

    // Rules.
    const rules = /^\/api\/v1\/admin\/(path-mappings|merge-modes|host-tiers|folder-mappings|notifications)(?:\/([^/]+))?$/.exec(path);
    if (rules) {
      const kind = rules[1];
      const id = rules[2] === undefined ? undefined : decodeURIComponent(rules[2]);
      if (id === undefined) {
        if (kind === "path-mappings") return page(world.paths);
        if (kind === "merge-modes") return page(world.merge);
        if (kind === "host-tiers") return page(world.tiers);
        if (kind === "folder-mappings") return page(world.folders);
        return page(world.channels);
      }
      if (call.method === "DELETE") {
        const known = (list: readonly { id: string }[]): boolean => list.some((x) => x.id === id);
        if (kind === "path-mappings") {
          if (!known(world.paths)) return notFound();
          world.paths = world.paths.filter((x) => x.id !== id);
        } else if (kind === "merge-modes") {
          if (!known(world.merge)) return notFound();
          world.merge = world.merge.filter((x) => x.id !== id);
        } else if (kind === "host-tiers") {
          if (!known(world.tiers)) return notFound();
          world.tiers = world.tiers.filter((x) => x.id !== id);
        } else if (kind === "folder-mappings") {
          if (!known(world.folders)) return notFound();
          world.folders = world.folders.filter((x) => x.id !== id);
        } else {
          if (!known(world.channels)) return notFound();
          world.channels = world.channels.filter((x) => x.id !== id);
        }
        return noContent();
      }
      if (call.method === "PUT") {
        const input = body(call);
        if (kind === "path-mappings") {
          const rule = { id, ...input } as PathMappingRule;
          world.paths = upsert(world.paths, rule);
          return json(rule);
        }
        if (kind === "merge-modes") {
          const rule = { id, ...input } as MergeModeRule;
          world.merge = upsert(world.merge, rule);
          return json(rule);
        }
        if (kind === "host-tiers") {
          const rule = { id, ...input } as HostTier;
          world.tiers = upsert(world.tiers, rule);
          return json(rule);
        }
        if (kind === "folder-mappings") {
          const rule = { id, ...input } as FolderMapping;
          world.folders = upsert(world.folders, rule);
          return json(rule);
        }
        // A channel: the webhook URL goes in and never comes out (S21); only whether one is set does.
        const { webhook_url, ...rest } = input;
        if (typeof webhook_url === "string") world.webhooks[id] = webhook_url;
        const existing = world.channels.find((c) => c.id === id);
        const channel = { id, ...rest, webhook_configured: typeof webhook_url === "string" || existing?.webhook_configured === true } as NotificationChannel;
        world.channels = upsert(world.channels, channel);
        return json(channel);
      }
    }

    // The signed-in person's own MCP tokens (the Admin "MCP" section shows these).
    const tokens = /^\/api\/v1\/me\/mcp-tokens(?:\/(\d+))?$/.exec(path);
    if (tokens) {
      if (world.mcp === null) return notFound();
      if (tokens[1] !== undefined) {
        world.mcp = world.mcp.map((t) => (String(t.id) === tokens[1] ? { ...t, revoked_at: NOW } : t));
        return noContent();
      }
      if (call.method === "POST") {
        const label = text(body(call)["label"]);
        const made: McpToken = { id: 100 + world.mcp.length, label, created_at: NOW, expires_at: NOW + 7 * 86_400_000, revoked_at: null, last_used_at: null };
        world.mcp = [made, ...world.mcp];
        return json({ ...made, token: world.mcpSecret }, 201);
      }
      return page(world.mcp);
    }
    return notFound();
  };
}
