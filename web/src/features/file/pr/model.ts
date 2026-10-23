import { ApiProblem } from "@/api/problem";

// The pure parts of the pull-request dialog (Consequence dialogs, 1a to 1e and the token answers): the title and the description it
// starts with, and what a GitHub token problem means. The job's steps, the open pull request and a pull request's address are the
// shared ones (src/components/pr, src/features/prs).

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
