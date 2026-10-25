import type { LucideIcon } from "lucide-react";
import type { ListProposalsParams, Proposal, ProposalStatus } from "@/api/generated/model";
import type { GateResult } from "@/components/app/write-gate";
import { Bell, CircleCheck, CircleDashed, CircleX, GitPullRequest, History, Hourglass, Pencil, RotateCw, Trash2, TriangleAlert, Upload, Zap } from "@/icons";
import { formatRelative } from "@/lib/time";

// What the approvals screen says about a proposal, as pure functions of the proposal the server sent. Nothing here is a fact the
// server did not give: the consequence sentence is built from the proposal's action and paths and claims no restart, no drift
// state and no effect that the contract does not carry (the design's chips for those need a preview the proposal has no field for).

/** The tabs of the page (H-2): the page file's four, not the handoff route's `pending|approved|rejected|withdrawn`. */
export const TABS = ["pending", "stale", "expired", "decided"] as const;
export type Tab = (typeof TABS)[number];

export const TAB_LABEL: Record<Tab, string> = { pending: "Pending", stale: "Stale", expired: "Expired", decided: "Decided" };

/** The tab the address names. The handoff route's approved and rejected are Decided; anything unknown (withdrawn: there is no such status) is Pending. */
export function tabOfSearch(state: string | undefined): Tab {
  switch (state) {
    case "stale":
    case "expired":
      return state;
    case "decided":
    case "approved":
    case "applied":
    case "rejected":
      return "decided";
    default:
      return "pending";
  }
}

/** The tab a proposal belongs to. */
export function tabOfStatus(status: ProposalStatus): Tab {
  switch (status) {
    case "stale":
    case "expired":
      return status;
    case "approved":
    case "applied":
    case "rejected":
      return "decided";
    default:
      return "pending";
  }
}

/**
 * The size of a page asked for. The hub clamps it to its own page (25 today, with their diffs), so this is only a ceiling. It is
 * the sidebar badge's number too, so Pending and the badge are ONE cache entry and one request.
 */
export const QUEUE_LIMIT = 100;

/** What each tab asks the server. Decided is the contract's `decided=true` (approved, applied and rejected): `decided` is not a status. */
export function queueParams(tab: Tab): Omit<ListProposalsParams, "cursor"> {
  switch (tab) {
    case "pending":
      return { status: "pending", limit: QUEUE_LIMIT };
    case "stale":
      return { status: "stale", limit: QUEUE_LIMIT };
    case "expired":
      return { status: "expired", limit: QUEUE_LIMIT };
    case "decided":
      return { decided: true, limit: QUEUE_LIMIT };
  }
}

export type ActionTone = "ui" | "cursor" | "delete" | "restart" | "pr" | "neutral";
export type ActionView = { readonly label: string; readonly tone: ActionTone; readonly Icon: LucideIcon };

/** The proposal's action as a chip. An edit made through MCP is the design's "AI-made edit". Without an `action` (an older hub) it is just a change. */
export function actionView(p: Pick<Proposal, "action" | "via">): ActionView {
  switch (p.action) {
    case "edit":
      return p.via === "mcp" ? { label: "AI-made edit", tone: "cursor", Icon: Zap } : { label: "Edit", tone: "ui", Icon: Pencil };
    case "upload":
      return { label: "Upload", tone: "ui", Icon: Upload };
    case "delete":
      return { label: "Delete", tone: "delete", Icon: Trash2 };
    case "revert":
      return { label: "Revert", tone: "ui", Icon: History };
    case "restart":
      return { label: "Restart", tone: "restart", Icon: RotateCw };
    case "pr":
      return { label: "Pull request", tone: "pr", Icon: GitPullRequest };
    case "notify":
      return { label: "Notify", tone: "restart", Icon: Bell };
    default:
      return { label: "Change", tone: "neutral", Icon: Pencil };
  }
}

export type StatusTone = "approval" | "stale" | "expired" | "ok" | "rejected" | "draft";
export type StatusView = { readonly label: string; readonly tone: StatusTone; readonly Icon: LucideIcon };

export function statusView(status: ProposalStatus): StatusView {
  switch (status) {
    case "pending":
      return { label: "Pending approval", tone: "approval", Icon: Hourglass };
    case "stale":
      return { label: "Stale", tone: "stale", Icon: TriangleAlert };
    case "expired":
      return { label: "Expired", tone: "expired", Icon: Hourglass };
    case "approved":
      return { label: "Approved", tone: "ok", Icon: CircleCheck };
    case "applied":
      return { label: "Applied", tone: "ok", Icon: CircleCheck };
    case "rejected":
      return { label: "Rejected", tone: "rejected", Icon: CircleX };
    case "draft":
      return { label: "Draft", tone: "draft", Icon: CircleDashed };
  }
}

/** "If you approve": one sentence, from the action and the paths. It says what the proposal does and no more. */
export function consequence(p: Proposal, lane: string): string {
  switch (p.status) {
    case "stale":
      return "Not approvable: the file changed since this was proposed.";
    case "expired":
      return `Would have changed ${lane}. It expired with no decision, so nothing was written.`;
    case "rejected":
      return `Nothing was written to ${lane}.`;
    case "applied":
      return `Written to ${lane} on approval.`;
    case "approved":
      return `Approved. The write to ${lane} follows the approval.`;
    default:
      break;
  }
  switch (p.action) {
    case "edit":
      return `Writes the proposed content to ${p.path} on ${lane}.`;
    case "upload":
      return `Replaces ${p.path} on ${lane} with the uploaded file.`;
    case "delete":
      return `Deletes ${p.path} from ${lane}.`;
    case "revert":
      return `Writes an earlier version back to ${p.path} on ${lane}.`;
    case "restart": {
      const services = p.paths ?? [p.path];
      return services.length === 1 ? `Restarts ${services[0] ?? p.path} on ${lane}.` : `Restarts ${services.length} services on ${lane}.`;
    }
    case "pr":
      return `Opens a pull request. Nothing changes on ${lane} until it merges.`;
    case "notify":
      return `Asks the config server to pick up the change on ${lane}.`;
    default:
      return `Applies the proposed change to ${lane}.`;
  }
}

/** What the File column and the drawer's title say: the file's name and its folder, or the services a restart names. */
export function fileParts(p: Pick<Proposal, "action" | "path" | "paths">): { name: string; where: string } {
  if (p.action === "restart") {
    const names = (p.paths ?? [p.path]).map((s) => s.slice(s.lastIndexOf("/") + 1));
    return { name: names.join(", "), where: "services/" };
  }
  const at = p.path.lastIndexOf("/");
  return at < 0 ? { name: p.path, where: "" } : { name: p.path.slice(at + 1), where: `${p.path.slice(0, at)}/` };
}

/** The drawer's title: "Edit core.yml", "AI-made edit to core.yml", "Restart gateway, core". */
export function titleOf(p: Pick<Proposal, "action" | "via" | "path" | "paths">): string {
  const { name } = fileParts(p);
  switch (p.action) {
    case "edit":
      return p.via === "mcp" ? `AI-made edit to ${name}` : `Edit ${name}`;
    case "upload":
      return `Upload ${name}`;
    case "delete":
      return `Delete ${name}`;
    case "revert":
      return `Revert ${name}`;
    case "restart":
      return `Restart ${name}`;
    case "pr":
      return `Pull request for ${name}`;
    case "notify":
      return `Notify for ${name}`;
    default:
      return `Change to ${name}`;
  }
}

const HOUR = 3_600_000;

export type Expiry = { readonly text: string; readonly urgent: boolean; readonly over: boolean };

/** "23 h", "40 min", "6 h ago". The last six hours are urgent (the design marks the 5 h row). */
export function expiresIn(p: Pick<Proposal, "expires_at">, now: number): Expiry {
  const left = p.expires_at - now;
  if (left <= 0) return { text: formatRelative(p.expires_at, now), urgent: false, over: true };
  const text = formatRelative(p.expires_at, now).replace(/^in /, "");
  return { text: text === "just now" ? "now" : text, urgent: left < 6 * HOUR, over: false };
}

/** How long a proposal lives, in hours, from the proposal itself (D37 says 72; the design says 24). Null when it makes no sense. */
export function lifetimeHours(p: Pick<Proposal, "created_at" | "expires_at">): number | null {
  const ms = p.expires_at - p.created_at;
  return ms > 0 ? Math.round(ms / HOUR) : null;
}

export type Decision = { readonly by: string | null; readonly verb: "Approved" | "Rejected"; readonly reason: string | null; readonly at: number | null };

/** Who decided, and what they said. Null for a proposal nobody has decided. */
export function decisionLine(p: Proposal): Decision | null {
  if (p.status !== "approved" && p.status !== "applied" && p.status !== "rejected") return null;
  return { by: p.decided_by, verb: p.status === "rejected" ? "Rejected" : "Approved", reason: p.decision_comment, at: p.decided_at ?? null };
}

export type DecideNote = "own" | "stale" | "expired" | "decided" | null;
export type Decide = { readonly approve: GateResult; readonly reject: GateResult; readonly note: DecideNote };

const yes: GateResult = { allowed: true, reason: null };
const no = (reason: string): GateResult => ({ allowed: false, reason });

/**
 * Whether the signed-in Admin may approve or reject this proposal, and the reason when not. The server is the authority (it answers
 * 403 for your own proposal and 422 `not_allowed` for one that is not pending); this only keeps a button from offering what will
 * be refused, and says why.
 */
export function canDecide(p: Proposal, me: string | null): Decide {
  const own = me !== null && p.author === me;
  if (p.status === "pending" || p.status === "stale") {
    if (own) return { approve: no("You can't approve your own proposal"), reject: no("You can't reject your own proposal"), note: "own" };
    if (p.status === "stale") return { approve: no("The file changed since this was proposed"), reject: yes, note: "stale" };
    return { approve: yes, reject: yes, note: null };
  }
  if (p.status === "expired") return { approve: no("Expired proposals can't be decided"), reject: no("Expired proposals can't be decided"), note: "expired" };
  if (p.status === "draft") return { approve: no("Not sent for approval yet"), reject: no("Not sent for approval yet"), note: null };
  return { approve: no("Already decided"), reject: no("Already decided"), note: "decided" };
}
