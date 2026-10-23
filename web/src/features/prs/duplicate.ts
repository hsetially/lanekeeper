import type { PrLink } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";

// The duplicate-pull-request answer (D77, CG13). `createPr` answers 409 for a second pull request on a change that already has an
// open one: `DuplicatePrProblem`, whose `reason` is `duplicate_pr` and whose `link` is the open one. The 409 is also the answer
// to a stale hash (`current`), so the app branches on `reason`, which is a code, and never on the wording of `title` or `detail`.
//
// `link` comes from the wire. Its members are checked for type and size before anything is drawn from it (a malformed link is
// "no link", and the caller shows the ordinary 409 instead), and members the contract does not have are dropped.

const SWIMLANE = /^[a-z0-9][a-z0-9-]{0,62}$/;
const STATES: readonly string[] = ["open", "merged", "closed"];

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const isWhole = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value);
const isText = (value: unknown, max: number): value is string => typeof value === "string" && value.length >= 1 && value.length <= max;

/** A `PrLink` from the wire, or null if any member is wrong. */
export function parsePrLink(value: unknown): PrLink | null {
  if (!isRecord(value)) return null;
  const { id, swimlane, path, nfs_hash, state, pr_url, pr_number, author, created_at, closed_at } = value;
  if (!isWhole(id) || id < 1) return null;
  if (typeof swimlane !== "string" || !SWIMLANE.test(swimlane)) return null;
  if (!isText(path, 1024)) return null;
  if (!isText(nfs_hash, 128)) return null;
  if (typeof state !== "string" || !STATES.includes(state)) return null;
  if (!isText(pr_url, 512)) return null;
  if (!isWhole(pr_number) || pr_number < 1) return null;
  if (!isText(author, 128)) return null;
  if (!isWhole(created_at)) return null;
  const closed = closed_at === null ? null : isWhole(closed_at) ? closed_at : undefined;
  if (closed === undefined) return null;
  return { id, swimlane, path, nfs_hash, state: state as PrLink["state"], pr_url, pr_number, author, created_at, closed_at: closed };
}

/** The open pull request that a 409 `duplicate_pr` names, or null if `error` is anything else. */
export function duplicatePrOf(error: unknown): PrLink | null {
  if (!(error instanceof ApiProblem) || error.status !== 409 || error.body["reason"] !== "duplicate_pr") return null;
  return parsePrLink(error.body["link"]);
}
