import type { LucideIcon } from "lucide-react";
import { ArrowLeftRight, Bell, Folder, Hourglass, Layers, Radio, Server, ShieldAlert, Split, Tag, User, Zap } from "@/icons";
import type { ADMIN_SECTIONS } from "@/routes/table";
import { linkTo } from "@/routes/links";

// The Admin area's own navigation (the left list of the Admin page): four groups of sections. Two sections are screens of their
// own (`/admin/users`, `/admin/swimlanes`); the others are `/admin/:section`. Four of the twelve (sentinels, OS Login, severity
// rules, retention) are built by task T5, not this one: they are in the list, so the area looks as designed and a link to one
// opens its page, which says it is not available yet until T5 adds it (`later`).

export type AdminArea = "users" | "swimlanes" | (typeof ADMIN_SECTIONS)[number];

export type NavItem = {
  readonly id: AdminArea;
  readonly label: string;
  readonly icon: LucideIcon;
  /** Built by a later task of the plan (T5). */
  readonly later?: true;
};

export type NavGroup = { readonly label: string; readonly items: readonly NavItem[] };

export const ADMIN_NAV: readonly NavGroup[] = [
  { label: "Access", items: [{ id: "users", label: "Users and access requests", icon: User }] },
  {
    label: "Infrastructure",
    items: [
      { id: "swimlanes", label: "Swimlanes and agents", icon: Layers },
      { id: "sentinels", label: "Sentinels", icon: Radio, later: true },
      { id: "tiers", label: "Host tiers", icon: Tag },
    ],
  },
  {
    label: "Mapping and rules",
    items: [
      { id: "paths", label: "Path mappings", icon: ArrowLeftRight },
      { id: "merge", label: "Merge modes", icon: Split },
      { id: "severity", label: "Severity rules", icon: ShieldAlert, later: true },
      { id: "folders", label: "Folder mappings", icon: Folder },
      { id: "oslogin", label: "OS Login users", icon: Server, later: true },
    ],
  },
  {
    label: "Data and alerts",
    items: [
      { id: "retention", label: "Retention", icon: Hourglass, later: true },
      { id: "notify", label: "Notifications", icon: Bell },
      { id: "mcp", label: "MCP personal tokens", icon: Zap },
    ],
  },
];

export const ADMIN_AREAS: readonly AdminArea[] = ADMIN_NAV.flatMap((group) => group.items.map((item) => item.id));

export function isAdminArea(value: string): value is AdminArea {
  return (ADMIN_AREAS as readonly string[]).includes(value);
}

/** The heading and the one line under it, as the design words each section. */
const COPY: Readonly<Record<AdminArea, { readonly title: string; readonly lead: string }>> = {
  users: { title: "Users and access requests", lead: "Approve new sign-ins, set roles, and choose whose changes need an Admin's approval." },
  swimlanes: { title: "Swimlanes and agents", lead: "Check that each swimlane's agent is healthy, and set its tenants, its auto-notify switch and the service account its agent runs as." },
  paths: { title: "Path mappings", lead: "Where the base files and the tenant files live in their Git repositories, so a Git file and its NFS file are matched, and back." },
  merge: { title: "Merge modes", lead: "Whether a tenant file replaces its base file or merges into it, decided by file path. The first matching rule wins." },
  tiers: { title: "Host tiers", lead: "Classify hostnames found in config values as dev, uat or prod, so findings can flag a prod host in a SIT file." },
  folders: { title: "Folder mappings", lead: "Link each NFS folder to the Deployments that read it, so a restart targets the right service." },
  notify: { title: "Notifications", lead: "Send events to Microsoft Teams. Each channel has one webhook and says which events it carries." },
  mcp: { title: "MCP personal tokens", lead: "Tokens that let a Cursor agent act as a user. The agent proposes changes; an Admin still approves them." },
  sentinels: { title: "Sentinels", lead: "The agent on each NFS VM that records who edits files. A missing heartbeat means edits go unrecorded." },
  oslogin: { title: "OS Login user mapping", lead: "Match the Linux account that edited a file on a VM to a Lanekeeper user. Matched by email, with manual overrides." },
  severity: { title: "Severity rules", lead: "Set a finding's severity by path, change type and finding type. The first matching rule wins." },
  retention: { title: "Retention", lead: "How long file versions are kept, and what a purge would delete before it runs." },
};

export function areaCopy(area: AdminArea): { readonly title: string; readonly lead: string } {
  return COPY[area];
}

/** The address of a section, from the route table (`/admin/users`, `/admin/swimlanes`, `/admin/paths`, ...). */
export function areaHref(area: AdminArea): string {
  if (area === "users") return linkTo("admin-users");
  if (area === "swimlanes") return linkTo("admin-swimlanes");
  return linkTo("admin-section", { section: area });
}
