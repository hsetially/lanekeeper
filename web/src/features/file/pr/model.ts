import type { PrJob, PrJobState, PrLink } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";

// The pure parts of the pull-request dialog (Consequence dialogs, 1a to 1k).

const MAX_TITLE = 256;

export type TitleInput = { target: "tenant" | "base"; stem: string; branch: string | null; lane: string };

/**
 * The title a PR starts with: "Update tx-infinity-core for sit7 (from sitb)" or "Promote tx-infinity-core from sit7 to base"; a base
 * file has no branch to promote from, so it reads "Update tx-infinity-core in base (from sitb)".
 */
export function defaultTitle({ target, stem, branch, lane }: TitleInput): string {
  const title = target === "tenant" ? `Update ${stem} for ${branch ?? "the tenant branch"} (from ${lane})` : branch === null ? `Update ${stem} in base (from ${lane})` : `Promote ${stem} from ${branch} to base`;
  return Array.from(title).slice(0, MAX_TITLE).join("");
}

export type BodyInput = { lane: string; path: string; repo: string; base: string; writes: string; hash: string };

/** The description a PR starts with: the swimlane, the file, where it goes and the NFS hash (twelve characters of it). */
export function defaultBody({ lane, path, repo, base, writes, hash }: BodyInput): string {
  return [
    `Raised from Lanekeeper on swimlane ${lane}.`,
    "",
    `Swimlane: ${lane} (${repo}@${base})`,
    `File: ${path}`,
    `Target: ${repo} / ${writes}`,
    `NFS hash: ${Array.from(hash).slice(0, 12).join("")}`,
  ].join("\n");
}

export type JobStep = { label: string; status: "done" | "active" | "pending" };

const STEPS = ["Preparing", "Committing", "Opening pull request", "Done"] as const;
const ACTIVE: Record<PrJobState, number> = { pending: 0, preparing: 0, committing: 1, opening: 2, completed: 4, failed: -1 };

/**
 * The four steps of a job, from the state the server reports and from nothing else. A queued job waits at the first step; a
 * failed job has no step done or active, because the state does not say how far it got.
 */
export function jobSteps(state: PrJob["state"]): JobStep[] {
  const at = ACTIVE[state];
  return STEPS.map((label, i) => ({ label, status: at < 0 ? "pending" : i < at ? "done" : i === at ? "active" : "pending" }));
}

/** What a GitHub token problem means for the person, or null for any other problem. */
export function tokenTrouble(error: unknown): "expired" | "unusable" | "no_access" | null {
  if (!(error instanceof ApiProblem)) return null;
  switch (error.kind) {
    case "token_expired":
      return "expired";
    case "token_rejected":
    case "token_unreadable":
    case "classic_token":
    case "not_fine_grained_token":
    case "github_account_mismatch":
      return "unusable";
    case "token_missing_access":
      return "no_access";
    default:
      return null;
  }
}

/** The open PR that a 409 `duplicate_pr` names, if the link is whole. */
export function duplicateOf(error: ApiProblem): PrLink | null {
  if (error.status !== 409 || error.body["reason"] !== "duplicate_pr") return null;
  const link = error.body["link"];
  if (typeof link !== "object" || link === null) return null;
  const candidate = link as Record<string, unknown>;
  const ok = typeof candidate["pr_url"] === "string" && typeof candidate["pr_number"] === "number" && Number.isInteger(candidate["pr_number"]) && typeof candidate["id"] === "number";
  return ok ? (link as PrLink) : null;
}

/** A URL the dialog may make a link of: https, a host and no spaces. A link from the server is not trusted to be one. */
export function httpsUrl(value: string): string | null {
  if (!/^https:\/\/[^\s/]+(?:\/\S*)?$/.test(value)) return null;
  try {
    return new URL(value).protocol === "https:" ? value : null;
  } catch {
    return null;
  }
}

export function prNumberOf(url: string): number | null {
  const found = /\/pull\/(\d{1,9})\/?$/.exec(url);
  return found?.[1] === undefined ? null : Number(found[1]);
}
