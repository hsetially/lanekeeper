import type { LucideIcon } from "lucide-react";
import type { AdminListAuditParams, AuditEvent } from "@/api/generated/model";
import type { NameLookup } from "@/components/attribution";
import type { Tone } from "@/features/approvals/chips";
import type { People } from "@/features/approvals/people";
import { eventWho } from "@/features/attribution";
import { Bell, CircleCheck, CircleX, Eye, FileText, GitPullRequest, History, Hourglass, Key, LogOut, Pencil, PanelLeft, Pin, RefreshCw, RotateCw, Server, Settings, Trash2, TriangleAlert, Upload, User, Zap } from "@/icons";
import { hasControlChar } from "@/lib/text/control";
import type { SearchOf } from "@/routes/props";

// What the audit screen says about an event, as pure functions of the event the hub sent. Nothing here is a fact the hub did not
// give. The audit read carries the actor as an id, the way the event arrived (`ui`, `mcp`, `system`), an action as text, and an
// attribution (who changed a file, by what evidence, how sure). It carries no file hashes, no approval and no path: the screen does
// not draw columns for them (design/DEVIATIONS.md).

/** The most events asked for in one page. The hub allows 500; a screen reads a hundred and asks for more when the person does. */
export const PAGE_SIZE = 100;

type ActionSpec = { readonly label: string; readonly Icon: LucideIcon };

// The actions the hub writes (crates/ports/src/audit.rs, `AuditAction`, in snake case). The list is only for the words and the icon:
// an action that is not here is shown by its own name, and the Action filter takes any name.
const ACTIONS: Readonly<Record<string, ActionSpec>> = {
  file_edited: { label: "Edit", Icon: Pencil },
  file_uploaded: { label: "Upload", Icon: Upload },
  file_deleted: { label: "Delete", Icon: Trash2 },
  file_reverted: { label: "Revert", Icon: History },
  file_change_detected: { label: "Change detected", Icon: Eye },
  attribution_upgraded: { label: "Attribution upgraded", Icon: User },
  service_restarted: { label: "Restart", Icon: RotateCw },
  config_server_notified: { label: "Notify", Icon: Bell },
  pr_raised: { label: "Pull request", Icon: GitPullRequest },
  pr_state_changed: { label: "Pull request changed", Icon: GitPullRequest },
  proposal_created: { label: "Proposal created", Icon: Hourglass },
  proposal_approved: { label: "Approve", Icon: CircleCheck },
  proposal_rejected: { label: "Reject", Icon: CircleX },
  proposal_expired: { label: "Proposal expired", Icon: Hourglass },
  proposal_stale: { label: "Proposal stale", Icon: TriangleAlert },
  draft_applied: { label: "Draft applied", Icon: Pencil },
  baseline_adopted: { label: "Baseline adopted", Icon: CircleCheck },
  drift_marked_intentional: { label: "Drift marked intentional", Icon: Pin },
  drift_mark_cleared: { label: "Drift mark cleared", Icon: Pin },
  flagged_value_revealed: { label: "Reveal", Icon: Eye },
  role_changed: { label: "Role changed", Icon: User },
  user_status_changed: { label: "User status changed", Icon: User },
  access_requested: { label: "Access requested", Icon: User },
  access_approved: { label: "Access approved", Icon: CircleCheck },
  access_denied: { label: "Access denied", Icon: CircleX },
  github_token_stored: { label: "GitHub token stored", Icon: Key },
  github_token_deleted: { label: "GitHub token deleted", Icon: Key },
  doc_created: { label: "Doc created", Icon: FileText },
  doc_updated: { label: "Doc updated", Icon: FileText },
  doc_deleted: { label: "Doc deleted", Icon: FileText },
  agent_joined: { label: "Agent joined", Icon: Server },
  agent_certificate_renewed: { label: "Agent certificate renewed", Icon: Server },
  sentinel_enrolled: { label: "Sentinel enrolled", Icon: Server },
  settings_changed: { label: "Settings changed", Icon: Settings },
  auto_notify_changed: { label: "Auto-notify changed", Icon: Bell },
  session_signed_in: { label: "Signed in", Icon: LogOut },
  session_signed_out: { label: "Signed out", Icon: LogOut },
  mcp_token_created: { label: "MCP token created", Icon: Key },
  mcp_token_revoked: { label: "MCP token revoked", Icon: Key },
};

/** The names of the actions the screen knows, in alphabetical order, for the Action filter. */
export const KNOWN_ACTIONS: readonly string[] = Object.keys(ACTIONS).sort();

const humanise = (action: string): string => {
  const words = action.replaceAll(/[_-]+/g, " ").trim();
  return words === "" ? action : words.charAt(0).toUpperCase() + words.slice(1);
};

export function actionView(action: string): ActionSpec {
  return ACTIONS[action] ?? { label: humanise(action), Icon: History };
}

export type ViaView = { readonly label: string; readonly tone: Tone; readonly Icon: LucideIcon; readonly hint: string };

/** How the event arrived: the contract has UI, MCP and system. A system event is a sync, a change found on NFS, or the hub itself. */
export function viaView(event: Pick<AuditEvent, "via" | "attribution">): ViaView {
  switch (event.via) {
    case "ui":
      return { label: "UI", tone: "ui", Icon: PanelLeft, hint: "Made in the web app" };
    case "mcp":
      return { label: "Cursor", tone: "cursor", Icon: Zap, hint: "Made through the MCP server, by an agent for a person" };
    default:
      switch (event.attribution?.source) {
        case "sync_job":
          return { label: "Sync", tone: "sync", Icon: RefreshCw, hint: "Made by a sync job" };
        case "sentinel_login":
        case "nfs_client":
        case "fs_owner_hint":
        case "unknown":
          return { label: "Detected", tone: "detected", Icon: Eye, hint: "Found on the NFS volume, not made through Lanekeeper" };
        default:
          return { label: "System", tone: "neutral", Icon: Server, hint: "Made by Lanekeeper itself" };
      }
  }
}

/**
 * The name of a user for the shared attribution words: the list's name, or nothing, and then those words say "Lanekeeper user" and
 * the start of the id. (Who made an event, how sure the attribution is and the source's words are the shared `EventWho`,
 * `AttributionSourceCell` and `AttributionEvidence`, src/components/attribution and src/features/attribution.)
 */
export const nameLookup =
  (people: Pick<People, "user">): NameLookup =>
  (id) =>
    people.user(id)?.display_name;

// ---- the filters ----

export type Filters = { actor?: string; action?: string; swimlane?: string; from?: string; to?: string };

const USER_ID = /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}:[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const SWIMLANE_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;
const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;
const DAY_MS = 86_400_000;

/** The start of a `YYYY-MM-DD` day in UTC, or null for text that is not a real day. */
function dayStart(text: string): number | null {
  const m = DAY.exec(text);
  if (m === null) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const ms = Date.UTC(y, mo - 1, d);
  const back = new Date(ms);
  return back.getUTCFullYear() === y && back.getUTCMonth() === mo - 1 && back.getUTCDate() === d ? ms : null;
}

/** The keys of the address the filters come from. */
export type RawSearch = { readonly [K in "actor" | "action" | "swimlane" | "from" | "to"]?: string | undefined };

/** What the address asks for, with everything the hub would refuse left out: a hand-edited address must not make a request that fails. */
export function filtersFromSearch(search: RawSearch): Filters {
  const out: Filters = {};
  if (search.actor !== undefined && USER_ID.test(search.actor)) out.actor = search.actor;
  if (search.swimlane !== undefined && SWIMLANE_ID.test(search.swimlane)) out.swimlane = search.swimlane;
  if (search.action !== undefined && search.action.length >= 1 && search.action.length <= 256 && !hasControlChar(search.action)) out.action = search.action;
  if (search.from !== undefined && dayStart(search.from) !== null) out.from = search.from;
  if (search.to !== undefined && dayStart(search.to) !== null) out.to = search.to;
  return out;
}

/** The question for the hub: a day is its first and last millisecond in UTC. */
export function auditParams(filters: Filters): Omit<AdminListAuditParams, "cursor"> {
  const from = filters.from === undefined ? null : dayStart(filters.from);
  const to = filters.to === undefined ? null : dayStart(filters.to);
  return {
    ...(filters.actor === undefined ? {} : { actor: filters.actor }),
    ...(filters.action === undefined ? {} : { action: filters.action }),
    ...(filters.swimlane === undefined ? {} : { swimlane: filters.swimlane }),
    ...(from === null ? {} : { from }),
    ...(to === null ? {} : { to: to + DAY_MS - 1 }),
    limit: PAGE_SIZE,
  };
}

/** The filters as the route's search keys, none of them empty. */
export function searchOf(filters: Filters): SearchOf<"audit"> {
  return {
    ...(filters.actor === undefined ? {} : { actor: filters.actor }),
    ...(filters.swimlane === undefined ? {} : { swimlane: filters.swimlane }),
    ...(filters.action === undefined ? {} : { action: filters.action }),
    ...(filters.from === undefined ? {} : { from: filters.from }),
    ...(filters.to === undefined ? {} : { to: filters.to }),
  };
}

// ---- the quick filters, on what is loaded ----

export type Quick = "all" | "cursor" | "unknown";

const isCursor = (e: AuditEvent): boolean => e.via === "mcp";
const isUnknown = (e: AuditEvent): boolean => eventWho(e).kind === "unknown";

export function quickCounts(events: readonly AuditEvent[]): Record<Quick, number> {
  return { all: events.length, cursor: events.filter(isCursor).length, unknown: events.filter(isUnknown).length };
}

export function applyQuick(events: readonly AuditEvent[], quick: Quick): readonly AuditEvent[] {
  if (quick === "cursor") return events.filter(isCursor);
  if (quick === "unknown") return events.filter(isUnknown);
  return events;
}

// ---- order and the link between events ----

export function newestFirst(events: readonly AuditEvent[]): AuditEvent[] {
  return [...events].sort((a, b) => b.seq - a.seq);
}

export type Break = { readonly stored: string; readonly actual: string; readonly previous: number };
export type Chain = { readonly checked: number; readonly loaded: number; readonly breaks: ReadonlyMap<number, Break> };

/**
 * Compare each event's stored previous hash with the hash of the event before it, for events that follow each other in what is
 * loaded. This compares two values the hub sent; it does not recompute a hash and it is not the hub's verification. A gap in
 * what is loaded is not judged. `events` must be newest first.
 */
export function chainOf(events: readonly AuditEvent[]): Chain {
  const breaks = new Map<number, Break>();
  let checked = 0;
  for (let i = 0; i + 1 < events.length; i++) {
    const newer = events[i];
    const older = events[i + 1];
    if (newer === undefined || older === undefined || newer.seq !== older.seq + 1) continue;
    checked += 1;
    if (newer.prev_hash !== older.hash) breaks.set(newer.id, { stored: newer.prev_hash, actual: older.hash, previous: older.seq });
  }
  return { checked, loaded: events.length, breaks };
}

// ---- the diff text ----

export type DiffKind = "meta" | "hunk" | "added" | "removed" | "context";
export type AuditDiffLine = { readonly kind: DiffKind; readonly text: string };

/** The lines of a diff text the hub kept with the event, cut at `max`. The kinds are for the colour; the text is shown as it is. */
export function diffLines(diff: string, max: number): { lines: AuditDiffLine[]; more: number } {
  const all = diff.split("\n");
  if (all[all.length - 1] === "") all.pop();
  const lines = all.slice(0, max).map((text): AuditDiffLine => {
    if (text.startsWith("+++") || text.startsWith("---") || text.startsWith("diff ") || text.startsWith("index ")) return { kind: "meta", text };
    if (text.startsWith("@@")) return { kind: "hunk", text };
    if (text.startsWith("+")) return { kind: "added", text };
    if (text.startsWith("-")) return { kind: "removed", text };
    return { kind: "context", text };
  });
  return { lines, more: Math.max(0, all.length - lines.length) };
}
