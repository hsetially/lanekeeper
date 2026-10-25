import type { Proposal } from "@/api/generated/model";
import { formatRelative } from "@/lib/time";
import { consequence, decisionLine, expiresIn } from "./model";

// The banner on top of the drawer: the one sentence that says where the proposal stands, before the diff. It is decided from the
// proposal the server last sent, and from the server's own answer to a decision the person made here (never from a guess about
// what the decision will do). Pure, so that the words can be read in a test.

export type BannerVariant = "info" | "success" | "warning" | "danger" | "lock";
export type Banner = { readonly variant: BannerVariant; readonly title: string; readonly body: string | null; /** A proposal to link to: the one an approval was sent on as. */ readonly request: number | null };

/** What the server answered to a decision made in this drawer. */
export type Answer = { readonly kind: "approved"; readonly audit: number; readonly warnings: number } | { readonly kind: "sent"; readonly id: number };

type Input = {
  p: Proposal;
  me: string | null;
  /** What to call a user id. */
  name: (id: string | null) => string;
  /** The names of the other active Admins. */
  others: readonly string[];
  now: number;
  answer: Answer | null;
  /** The drawer showed this proposal while it was pending: a decision that arrives now was made while the person had it open. */
  sawPending: boolean;
};

const MOST = 4;

/** "A and B are the other Admins." */
export function otherAdminsSentence(names: readonly string[]): string {
  if (names.length === 0) return "";
  if (names.length === 1) return `${names[0] ?? ""} is the other Admin.`;
  const shown = names.slice(0, MOST + 1 === names.length ? names.length : MOST);
  const more = names.length - shown.length;
  const head = shown.slice(0, -1).join(", ");
  const last = more > 0 ? `${shown[shown.length - 1] ?? ""} and ${more} more` : shown[shown.length - 1] ?? "";
  const lead = more > 0 ? `${head}, ${last}` : `${head} and ${last}`;
  return `${lead} are the other Admins.`;
}

const make = (variant: BannerVariant, title: string, body: string | null, request: number | null = null): Banner => ({ variant, title, body, request });

export function bannerOf({ p, me, name, others, now, answer, sawPending }: Input): Banner | null {
  if (answer?.kind === "approved") {
    const noted = answer.warnings > 0 ? ` The server noted ${answer.warnings} ${answer.warnings === 1 ? "thing" : "things"} worth knowing.` : "";
    return make("success", `Approved · written to ${p.swimlane}`, `Audit event ${answer.audit.toLocaleString("en-US")}.${noted}`);
  }
  if (answer?.kind === "sent") return make("info", "Sent for approval", `Your approval is not applied until another Admin approves request ${answer.id}.`, answer.id);

  const decision = decisionLine(p);
  if (decision !== null) {
    const who = decision.by === null ? "Lanekeeper" : name(decision.by);
    const when = decision.at === null ? "" : `, ${formatRelative(decision.at, now)}`;
    if (decision.verb === "Rejected") return make("danger", `Rejected by ${who}${when}`, decision.reason !== null && decision.reason !== "" ? `“${decision.reason}”` : `Nothing was written to ${p.swimlane}.`);
    const elsewhere = sawPending && decision.by !== me ? "Someone else decided while you had it open. " : "";
    return make("success", `Approved by ${who}${when}`, `${elsewhere}${consequence(p, p.swimlane)}`);
  }

  const own = me !== null && p.author === me;
  if (own && (p.status === "pending" || p.status === "stale")) {
    const rest = otherAdminsSentence(others);
    return make("lock", "You can't approve your own proposal", `Another Admin has to approve it, so no one can change a swimlane alone.${rest === "" ? "" : ` ${rest}`}`);
  }
  if (p.status === "stale")
    return make("warning", "The file changed since this was proposed", `Approving would overwrite the newer file. Reject this proposal with a reason, and ${name(p.author)} can propose it again against the current file.`);
  if (p.status === "expired") return make("info", `Expired ${expiresIn(p, now).text} with no decision`, `Nothing was written to ${p.swimlane}. ${name(p.author)} can propose it again.`);
  return null;
}
