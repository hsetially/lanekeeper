import type { LucideIcon } from "lucide-react";
import { Role, type DocHit, type SettingHit, type SwimlaneSummary, type User } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { BookOpen, CircleCheck, Columns2, FileCode, History, Key, Keyboard, Layers, PanelLeft, Search, Settings, ShieldAlert, User as UserIcon, Zap } from "@/icons";
import type { ShortcutId } from "@/keyboard";
import { LinkError, hrefFor, linkTo } from "@/routes/links";
import type { RouteId } from "@/routes/props";
import { routeById } from "@/routes/table";
import { score } from "./match";

// The command palette as data (the Shell, frames 1c and 1d). One table says what each group is: where its results come from,
// the least role that gets it, how long it waits after a keystroke, how many it shows, and where its results are kept. The
// builder turns a query and what the groups returned into the list on screen, in the order ⌘1 to ⌘9 count it. It is pure, so
// the rules (role filtering, the `>` prefix, the cap, nothing that would be a broken link) are read from tests, not from a
// browser.
//
// Files and Services, which the design has, are not here: no operation searches every swimlane for either (CG16), and a
// palette that listed one swimlane's files would say something untrue about the others (design/DEVIATIONS.md).

export type PaletteGroupId = "swimlanes" | "settings" | "docs" | "users" | "actions";

export type PaletteGroupSpec = {
  readonly id: PaletteGroupId;
  readonly title: string;
  /** The generated operation behind it, or `static`. */
  readonly operation: "listSwimlanes" | "searchSettings" | "searchDocs" | "adminListUsers" | "static";
  readonly minRole: Role;
  /** Milliseconds to wait after the last keystroke before asking the server. */
  readonly debounceMs: number;
  /** The most results shown. */
  readonly cap: number;
  /** Where results are kept: the app's query cache, memory for as long as the palette is open, or nowhere. */
  readonly cache: string;
  /** The shortest query that is sent to the server. */
  readonly minQuery: number;
};

export const GROUP: Record<PaletteGroupId, PaletteGroupSpec> = {
  swimlanes: { id: "swimlanes", title: "Swimlanes", operation: "listSwimlanes", minRole: Role.viewer, debounceMs: 0, cap: 8, cache: "query cache: the swimlane list, shared with the sidebar", minQuery: 0 },
  settings: { id: "settings", title: "Settings", operation: "searchSettings", minRole: Role.viewer, debounceMs: 200, cap: 8, cache: "memory while the palette is open (a result is not kept in the query cache)", minQuery: 2 },
  docs: { id: "docs", title: "Docs", operation: "searchDocs", minRole: Role.viewer, debounceMs: 200, cap: 8, cache: "memory while the palette is open", minQuery: 2 },
  users: { id: "users", title: "Users", operation: "adminListUsers", minRole: Role.admin, debounceMs: 200, cap: 8, cache: "memory while the palette is open", minQuery: 2 },
  actions: { id: "actions", title: "Actions", operation: "static", minRole: Role.viewer, debounceMs: 0, cap: 8, cache: "none: a fixed list", minQuery: 0 },
};

/** The groups in the order they appear on screen. */
export const PALETTE_GROUPS: readonly PaletteGroupSpec[] = [GROUP.swimlanes, GROUP.settings, GROUP.docs, GROUP.users, GROUP.actions];

export type PaletteAction =
  | { readonly kind: "go"; readonly href: string; /** The swimlane to remember as the current one. */ readonly swimlane?: string }
  | { readonly kind: "ui"; readonly what: "shortcuts" | "sidebar" | "theme-light" | "theme-dark" | "theme-system" };

export type PaletteItem = {
  /** `group:key`, unique in the list. */
  readonly id: string;
  readonly label: string;
  /** The label is a name from the file system or the registry: drawn in the mono face. */
  readonly mono: boolean;
  /** A muted line beside the label. */
  readonly meta?: string;
  /** A small chip at the end (the tier of a swimlane). */
  readonly chip?: string;
  readonly icon: LucideIcon;
  readonly action: PaletteAction;
  /** The shortcut of the same thing, drawn at the end of the row. */
  readonly hint?: { readonly row: ShortcutId; readonly binding: number };
};

export type Remote<T> = { readonly status: "idle" | "loading" | "ready" | "error"; readonly items: readonly T[] };

export type PaletteInput = {
  readonly query: string;
  readonly role: Role | null;
  readonly swimlanes: readonly SwimlaneSummary[];
  readonly docs: Remote<DocHit>;
  readonly settings: Remote<SettingHit>;
  readonly users: Remote<User>;
};

export type PaletteSection = { readonly spec: PaletteGroupSpec; readonly status: "ready" | "loading" | "error"; readonly items: readonly PaletteItem[] };
export type PaletteView = {
  /** The query began with `>`: only actions. */
  readonly actionsOnly: boolean;
  /** The text that is matched (the query without the `>`). */
  readonly text: string;
  readonly sections: readonly PaletteSection[];
  /** Every visible row in screen order. */
  readonly items: readonly PaletteItem[];
  /** The results ⌘1 to ⌘9 run, in screen order: the rows that are not actions (the design numbers results, not actions). */
  readonly numbered: readonly PaletteItem[];
};

/** ⌘1 to ⌘9. */
export const MAX_NUMBERED = 9;

// -- actions ---------------------------------------------------------------------------------------------------------------

type ActionDef = {
  readonly id: string;
  readonly label: string;
  readonly icon: LucideIcon;
  readonly minRole: Role;
  readonly keywords?: readonly string[];
  readonly run: PaletteAction;
  readonly hint?: { readonly row: ShortcutId; readonly binding: number };
};

/** The least role that may open a route, as a Role (a public route needs none). */
function roleFor(route: RouteId): Role {
  const access = routeById(route).access;
  return access === "public" ? Role.viewer : access;
}

const go = (id: string, label: string, icon: LucideIcon, route: RouteId, search?: Record<string, string>, hint?: ActionDef["hint"]): ActionDef => ({
  id,
  label,
  icon,
  minRole: roleFor(route),
  run: { kind: "go", href: search ? hrefFor(route, {}, search) : linkTo(route as "swimlanes") },
  ...(hint ? { hint } : {}),
});

const GO_TO = "go-to.swimlanes-compare-feature-status-findings" as const;

export const ACTIONS: readonly ActionDef[] = [
  go("go-swimlanes", "Go to Swimlanes", Layers, "swimlanes", undefined, { row: GO_TO, binding: 0 }),
  go("go-compare", "Go to Compare", Columns2, "compare", undefined, { row: GO_TO, binding: 1 }),
  go("go-status", "Go to Feature status", Zap, "status", undefined, { row: GO_TO, binding: 2 }),
  go("go-findings", "Go to Findings", ShieldAlert, "findings", undefined, { row: GO_TO, binding: 3 }),
  go("go-approvals", "Go to Approvals", CircleCheck, "approvals"),
  go("go-audit", "Go to Audit", History, "audit", undefined, { row: GO_TO, binding: 4 }),
  go("go-admin", "Go to Admin", Settings, "admin-users"),
  go("search-text", "Search text…", Search, "search", { type: "text" }, { row: "global.search-text-focus-query", binding: 0 }),
  go("search-settings", "Search settings…", Search, "search", { type: "settings" }),
  go("search-docs", "Search docs…", BookOpen, "search", { type: "docs" }),
  go("github-token", "My GitHub token", Key, "github-token"),
  { id: "shortcuts", label: "Keyboard shortcuts", icon: Keyboard, minRole: Role.viewer, keywords: ["keys", "help"], run: { kind: "ui", what: "shortcuts" }, hint: { row: "global.shortcuts-sheet", binding: 0 } },
  { id: "sidebar", label: "Toggle sidebar", icon: PanelLeft, minRole: Role.viewer, run: { kind: "ui", what: "sidebar" }, hint: { row: "global.toggle-sidebar", binding: 0 } },
  { id: "theme-light", label: "Theme: Light", icon: Settings, minRole: Role.viewer, keywords: ["appearance"], run: { kind: "ui", what: "theme-light" } },
  { id: "theme-dark", label: "Theme: Dark", icon: Settings, minRole: Role.viewer, keywords: ["appearance"], run: { kind: "ui", what: "theme-dark" } },
  { id: "theme-system", label: "Theme: System", icon: Settings, minRole: Role.viewer, keywords: ["appearance"], run: { kind: "ui", what: "theme-system" } },
];

// -- results ---------------------------------------------------------------------------------------------------------------

/** A link, or nothing: a value the router would refuse makes no result, never a link that goes somewhere else. */
function link(make: () => string): string | undefined {
  try {
    return make();
  } catch (error) {
    if (error instanceof LinkError) return undefined;
    throw error;
  }
}

const ANCHOR = /^[A-Za-z0-9_.:-]{1,128}$/;
const basename = (path: string): string => path.split("/").pop() ?? path;

function swimlaneItems(text: string, swimlanes: readonly SwimlaneSummary[]): PaletteItem[] {
  return swimlanes
    .map((s) => ({ s, rank: Math.max(score(text, s.display_name), score(text, s.id), ...s.tenants.map((t) => score(text, t) * 0.5)) }))
    .filter(({ rank }) => rank > 0)
    .sort((a, b) => b.rank - a.rank || a.s.display_name.localeCompare(b.s.display_name))
    .flatMap(({ s }): PaletteItem[] => {
      const href = link(() => hrefFor("swimlane", { id: s.id }));
      if (href === undefined) return [];
      const first = s.tenants[0];
      return [
        {
          id: `swimlanes:${s.id}`,
          label: s.display_name,
          mono: true,
          ...(first === undefined ? {} : { meta: s.tenants.length > 1 ? `${first} +${s.tenants.length - 1}` : first }),
          ...(s.tier ? { chip: s.tier.toUpperCase() } : {}),
          icon: Layers,
          action: { kind: "go", href, swimlane: s.id },
        },
      ];
    });
}

function settingItems(hits: readonly SettingHit[]): PaletteItem[] {
  return hits.flatMap((h, i): PaletteItem[] => {
    const href = link(() => hrefFor("files", { id: h.swimlane }, { path: h.file, line: String(h.location.lines.start) }));
    if (href === undefined) return [];
    // The value is not shown: a palette is a place to find, and a value the server did not recognise as secret must not be on screen by accident.
    return [{ id: `settings:${h.swimlane}:${h.file}:${h.path}:${i}`, label: h.path, mono: true, meta: `${h.swimlane} · ${basename(h.file)}`, icon: FileCode, action: { kind: "go", href } }];
  });
}

function docItems(hits: readonly DocHit[]): PaletteItem[] {
  return hits.flatMap((h): PaletteItem[] => {
    const base = link(() => hrefFor("doc-view", {}, { path: h.path }));
    if (base === undefined) return [];
    const anchor = h.anchor !== undefined && h.anchor !== null && ANCHOR.test(h.anchor) ? `#${h.anchor}` : "";
    const trail = h.heading_path.length > 0 ? h.heading_path.join(" › ") : h.path;
    return [{ id: `docs:${h.doc}:${h.path}`, label: h.title, mono: false, meta: trail, icon: BookOpen, action: { kind: "go", href: `${base}${anchor}` } }];
  });
}

function userItems(users: readonly User[]): PaletteItem[] {
  return users.flatMap((u): PaletteItem[] => {
    const href = link(() => hrefFor("admin-users", {}, { q: u.email }));
    if (href === undefined) return [];
    return [{ id: `users:${u.id}`, label: u.display_name, mono: false, meta: u.email, icon: UserIcon, action: { kind: "go", href } }];
  });
}

function actionItems(text: string, role: Role | null): PaletteItem[] {
  return ACTIONS.filter((a) => roleAtLeast(role, a.minRole))
    .map((a) => ({ a, rank: Math.max(score(text, a.label), ...(a.keywords ?? []).map((k) => score(text, k) * 0.5)) }))
    .filter(({ rank }) => rank > 0)
    .map(({ a }): PaletteItem => ({ id: `actions:${a.id}`, label: a.label, mono: false, icon: a.icon, action: a.run, ...(a.hint ? { hint: a.hint } : {}) }));
}

const spec = (id: PaletteGroupId): PaletteGroupSpec => GROUP[id];

export function buildPalette(input: PaletteInput): PaletteView {
  const { role } = input;
  if (role === null) return { actionsOnly: false, text: input.query.trim(), sections: [], items: [], numbered: [] };
  const actionsOnly = input.query.trimStart().startsWith(">");
  const text = (actionsOnly ? input.query.trimStart().slice(1) : input.query).trim();
  const sections: PaletteSection[] = [];
  const add = (group: PaletteGroupSpec, status: PaletteSection["status"], items: readonly PaletteItem[]): void => {
    if (status === "ready" && items.length === 0) return;
    sections.push({ spec: group, status, items: items.slice(0, group.cap) });
  };

  if (!actionsOnly) {
    add(spec("swimlanes"), "ready", swimlaneItems(text, input.swimlanes));
    const remote = <T,>(id: PaletteGroupId, state: Remote<T>, make: (items: readonly T[]) => PaletteItem[]): void => {
      const group = spec(id);
      if (!roleAtLeast(role, group.minRole) || text.length < group.minQuery || state.status === "idle") return;
      add(group, state.status === "loading" ? "loading" : state.status === "error" ? "error" : "ready", state.status === "ready" ? make(state.items) : []);
    };
    remote("settings", input.settings, settingItems);
    remote("docs", input.docs, docItems);
    remote("users", input.users, userItems);
  }
  add(spec("actions"), "ready", actionItems(text, role));

  const numbered = sections.filter((s) => s.spec.id !== "actions").flatMap((s) => s.items).slice(0, MAX_NUMBERED);
  return { actionsOnly, text, sections, items: sections.flatMap((s) => s.items), numbered };
}
