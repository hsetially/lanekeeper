import { LinkError, hrefFor } from "@/routes/links";
import { ROUTES } from "@/routes/table";

// The breadcrumb (the Shell header): where the person is, and a link to each place above it. A function of the route, its
// path parameters and its search, so it needs no state and a test can read it. A part of a path is the file system's, so it is
// untrusted text: the component draws it through SafeText, and a link is built only from values the route table accepts.

export type Crumb = { label: string; href?: string; mono?: boolean };

/** What the breadcrumb says for a swimlane's screens. */
const TAB: Record<string, string> = {
  services: "Services",
  drift: "Drift",
  "swimlane-findings": "Findings",
  "swimlane-audit": "Audit",
  activity: "Activity",
  "swimlane-pull-requests": "Pull requests",
  adopt: "Adopt this swimlane",
};

const SEARCH_TAB: Record<string, string> = { text: "Text", settings: "Settings", docs: "Docs" };

const ADMIN_SECTION: Record<string, string> = {
  users: "Users and access requests",
  swimlanes: "Swimlanes and agents",
  paths: "Path mappings",
  merge: "Merge modes",
  tiers: "Host tiers",
  folders: "Folder mappings",
  notify: "Notifications",
  mcp: "MCP personal tokens",
  sentinels: "Sentinels",
  oslogin: "OS Login user mapping",
  severity: "Severity rules",
  retention: "Retention",
};

/** The most crumbs a path gives: the first folders and the file. A path can be 4,096 characters deep. */
const MAX_CRUMBS = 12;

function link(route: string, params: Record<string, string> = {}, search: Record<string, string | undefined> = {}): string | undefined {
  try {
    return hrefFor(route, params, search);
  } catch (error) {
    if (error instanceof LinkError) return undefined;
    throw error;
  }
}

function withHref(label: string, href: string | undefined, mono = false): Crumb {
  return { label, ...(href === undefined ? {} : { href }), ...(mono ? { mono: true } : {}) };
}

const current = (label: string, mono = false): Crumb => ({ label, ...(mono ? { mono: true } : {}) });

function swimlaneCrumbs(id: string, linked: boolean): Crumb[] {
  const home = withHref("Swimlanes", link("swimlanes"));
  return linked ? [home, withHref(id, link("swimlane", { id }), true)] : [home, current(id, true)];
}

export function crumbsFor(routeId: string | undefined, params: Record<string, string>, search: Record<string, string | undefined>): Crumb[] {
  if (routeId === undefined) return [];
  const route = ROUTES.find((r) => r.id === routeId);
  if (!route) return [];
  const id = params["id"] ?? "";
  switch (routeId) {
    case "swimlane":
      return swimlaneCrumbs(id, false);
    case "files": {
      const path = search["path"];
      const trail = swimlaneCrumbs(id, true);
      if (path === undefined || path === "") return [...trail, current("Files")];
      const segments = path.split("/").filter((s) => s !== "");
      const shown = segments.length > MAX_CRUMBS - 3 ? segments.slice(0, MAX_CRUMBS - 4).concat(["…"], segments.slice(-1)) : segments;
      const crumbs = shown.map((segment, i): Crumb => {
        if (i === shown.length - 1) return current(segment, true);
        const prefix = segments.slice(0, i + 1).join("/");
        return segment === "…" ? current("…", true) : withHref(segment, link("files", { id }, { path: prefix }), true);
      });
      return [...trail, ...crumbs];
    }
    case "services":
    case "drift":
    case "swimlane-findings":
    case "swimlane-audit":
    case "activity":
    case "swimlane-pull-requests":
    case "adopt":
      return [...swimlaneCrumbs(id, true), current(TAB[routeId] ?? route.title)];
    case "search":
      return [withHref("Search", link("search")), current(SEARCH_TAB[search["type"] ?? "text"] ?? "Text")];
    case "doc":
    case "doc-view":
      return [withHref("Search", link("search", {}, { type: "docs" })), current("Docs")];
    case "approval":
      return [withHref("Approvals", link("approvals")), current(`#${id}`)];
    case "admin-users":
      return [withHref("Admin", link("admin-users")), current(ADMIN_SECTION["users"] ?? route.title)];
    case "admin-swimlanes":
      return [withHref("Admin", link("admin-users")), current(ADMIN_SECTION["swimlanes"] ?? route.title)];
    case "admin-section": {
      const section = params["section"] ?? "";
      return [withHref("Admin", link("admin-users")), current(ADMIN_SECTION[section] ?? route.title)];
    }
    default:
      return [current(route.title)];
  }
}
