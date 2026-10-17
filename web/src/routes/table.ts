// The route table: every URL of the app, who may open it, which search keys it reads, and where it came from. The router
// (src/router.tsx) is built from this table and from nothing else, so a route that is not here does not exist.
//
// Sources: `handoff` rows come from the Engineering Handoff page's screen inventory; `addition` rows are the approved
// additions of the plan (the paths the hub and MCP links already send users to, the Detail tabs the handoff has no route
// for, and the screens the contract added). src/routes/table.test.ts compares this file with the handoff data, and
// inventory.test.ts fails on any route that is in neither group, so adding a route is a reviewed change.
//
// Role differences from the handoff, decided in the plan (W11, W12): Audit and the Activity tab are Admin-only (the contract
// says so), and Approvals is Admin-only to open and decide.

import type { Role } from "@/api/generated/model";

/** `public` needs no session (and makes no API call); anything else is the minimum role. */
export type RouteAccess = "public" | Role;

export type RouteDef = {
  /** Stable name: used by `hrefFor`, by tests and by the keyboard registry. */
  id: string;
  /** TanStack style is `$id`; the table keeps the handoff's `:id`. */
  path: string;
  access: RouteAccess;
  /** The screen's name: the placeholder's h1 until the real screen replaces it, and the page title. */
  title: string;
  origin: "handoff" | "addition";
  /** The search keys the route reads, all strings. */
  search?: readonly string[];
  /** Where the route sends the visitor instead of showing a screen (aliases and server-issued links). */
  redirect?: { to: string; params?: Readonly<Record<string, string>> };
};

const SWIMLANE_FILTERS = ["q"] as const;

export const ROUTES: readonly RouteDef[] = [
  // Before the session (public: no useMe, no event stream, no 401 redirect).
  { id: "signin", path: "/signin", access: "public", title: "Sign in", origin: "handoff", search: ["next"] },
  { id: "access-pending", path: "/access-requested", access: "public", title: "Access requested", origin: "addition" },
  { id: "access-pending-alias", path: "/access/pending", access: "public", title: "Access requested", origin: "handoff", redirect: { to: "/access-requested" } },
  { id: "access-disabled", path: "/access-disabled", access: "public", title: "Access disabled", origin: "addition" },
  { id: "access-disabled-alias", path: "/access/disabled", access: "public", title: "Access disabled", origin: "addition", redirect: { to: "/access-disabled" } },
  // Account.
  { id: "onboarding", path: "/access/onboarding", access: "editor", title: "Connect your GitHub token", origin: "handoff" },
  { id: "github-token", path: "/me/github-token", access: "editor", title: "My GitHub token", origin: "addition" },
  { id: "mcp-tokens", path: "/me/tokens", access: "viewer", title: "Personal MCP tokens", origin: "addition" },
  // Swimlanes.
  { id: "swimlanes", path: "/swimlanes", access: "viewer", title: "Swimlanes", origin: "handoff", search: ["tier", "state", "q"] },
  { id: "swimlane", path: "/swimlanes/:id", access: "viewer", title: "Swimlane", origin: "handoff" },
  { id: "files", path: "/swimlanes/:id/files", access: "viewer", title: "Files", origin: "handoff", search: ["path", "line", "diff", "sort", "cursor", "tab", "panel"] },
  { id: "services", path: "/swimlanes/:id/services", access: "viewer", title: "Services", origin: "handoff", search: SWIMLANE_FILTERS },
  { id: "swimlane-findings", path: "/swimlanes/:id/findings", access: "viewer", title: "Findings", origin: "handoff", search: ["code", "severity", "kind"] },
  { id: "swimlane-audit", path: "/swimlanes/:id/audit", access: "admin", title: "Audit", origin: "handoff" },
  { id: "adopt", path: "/swimlanes/:id/adopt", access: "admin", title: "Adopt this swimlane", origin: "handoff", search: ["step"] },
  { id: "drift", path: "/swimlanes/:id/drift", access: "viewer", title: "Drift", origin: "addition" },
  { id: "activity", path: "/swimlanes/:id/activity", access: "admin", title: "Activity", origin: "addition" },
  // Across swimlanes.
  { id: "compare", path: "/compare", access: "viewer", title: "Compare", origin: "handoff", search: ["lanes", "file", "only", "channel"] },
  { id: "status", path: "/status", access: "viewer", title: "Feature status", origin: "handoff", search: ["setting", "lane"] },
  { id: "search", path: "/search", access: "viewer", title: "Search", origin: "handoff", search: ["type", "q", "cs", "rx", "lanes", "glob", "mode"] },
  { id: "doc", path: "/docs/:slug", access: "viewer", title: "Doc", origin: "handoff", search: ["version"] },
  { id: "doc-view", path: "/docs/view", access: "viewer", title: "Doc", origin: "addition", search: ["path"] },
  { id: "findings", path: "/findings", access: "viewer", title: "Findings", origin: "handoff", search: ["code", "severity", "lanes", "kind"] },
  { id: "approvals", path: "/approvals", access: "admin", title: "Approvals", origin: "handoff", search: ["state"] },
  { id: "approval", path: "/approvals/:id", access: "admin", title: "Approval", origin: "handoff" },
  { id: "proposal-link", path: "/proposals/:id", access: "admin", title: "Approval", origin: "addition", redirect: { to: "/approvals/:id" } },
  { id: "draft", path: "/drafts/:id", access: "editor", title: "Draft", origin: "addition" },
  { id: "audit", path: "/audit", access: "admin", title: "Audit", origin: "handoff", search: ["actor", "swimlane", "action", "from", "to", "cursor"] },
  // Admin.
  { id: "admin-users", path: "/admin/users", access: "admin", title: "Users and access requests", origin: "handoff", search: ["q", "role"] },
  { id: "admin-swimlanes", path: "/admin/swimlanes", access: "admin", title: "Swimlanes and agents", origin: "handoff" },
  { id: "admin-section", path: "/admin/:section", access: "admin", title: "Admin", origin: "handoff" },
  // Errors (public: they must render when nothing else can).
  { id: "not-found", path: "/404", access: "public", title: "Page not found", origin: "handoff", search: ["req"] },
  { id: "error", path: "/error", access: "public", title: "Something went wrong", origin: "handoff", search: ["req"] },
  { id: "forbidden", path: "/403", access: "public", title: "You do not have access", origin: "handoff", search: ["req"] },
];

/** The sections of `/admin/:section` (the handoff's five and the contract's five). */
export const ADMIN_SECTIONS = ["paths", "merge", "tiers", "folders", "notify", "mcp", "sentinels", "oslogin", "severity", "retention"] as const;

// Parameters are checked before a screen renders, because the generated client puts them into a URL path as they are.
const PARAM_CHECKS: Record<string, RegExp> = {
  // SwimlaneId in api/openapi.yaml.
  swimlane: /^[a-z0-9][a-z0-9-]{0,62}$/,
  // A proposal, draft or other numeric id (NumericId).
  numeric: /^[1-9][0-9]{0,17}$/,
  // A doc: an encoded token with no dots and no slashes (the hub answers a path with a dot by a JSON 404, not by the app).
  slug: /^[A-Za-z0-9_-]{1,2048}$/,
};

/** Which check a path parameter gets, by route and name. */
export function paramCheck(route: RouteDef, name: string): ((value: string) => boolean) | undefined {
  if (name === "section") return (v) => (ADMIN_SECTIONS as readonly string[]).includes(v);
  if (name === "slug") return (v) => PARAM_CHECKS["slug"]?.test(v) === true;
  if (name === "id") {
    const numeric = route.path.startsWith("/approvals/") || route.path.startsWith("/proposals/") || route.path.startsWith("/drafts/");
    const re = PARAM_CHECKS[numeric ? "numeric" : "swimlane"];
    return (v) => re?.test(v) === true;
  }
  return undefined;
}

export function routeById(id: string): RouteDef {
  const route = ROUTES.find((r) => r.id === id);
  if (!route) throw new Error(`no route ${id}`);
  return route;
}

/** The names of the `:params` in a path, in order. */
export function paramNames(path: string): string[] {
  return path
    .split("/")
    .filter((s) => s.startsWith(":"))
    .map((s) => s.slice(1));
}
