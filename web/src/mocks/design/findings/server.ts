// What the hub answers to `GET /api/v1/findings`, for the scenarios and the tests: the filters the contract has (one swimlane,
// the checks, the severity "at least"), the order the registry gives (newest first, by id: `ORDER BY id DESC`) and the keyset
// cursor over that order. A fake that filtered or ordered differently would let a screen rely on what the server does not do.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { Finding, FindingPage, Severity } from "../../../api/generated/model/index.ts";

const RANK: Readonly<Record<Severity, number>> = { low: 0, medium: 1, high: 2, critical: 3 };

/** The page size when the request names none, and the most it may name (the contract's `Limit`). */
export const DEFAULT_LIMIT = 100;
export const MAX_LIMIT = 500;

/** The cursor that follows the finding `id`. It reads like the contract's `Cursor` pattern. */
export const cursorAfter = (id: number): string => `after-${id}`;

function afterOf(cursor: string | null): number | null {
  const match = cursor === null ? null : /^after-([0-9]{1,18})$/.exec(cursor);
  return match?.[1] === undefined ? null : Number(match[1]);
}

/** One page of `all`, as the registry serves it for these query parameters. */
export function findingPage(all: readonly Finding[], query: URLSearchParams): FindingPage {
  const swimlane = query.get("swimlane");
  const kinds = (query.get("kinds") ?? "").split(",").filter((k) => k !== "");
  const asked = query.get("min_severity") ?? "low";
  const floor = (Object.keys(RANK) as Severity[]).find((severity) => severity === asked);
  const minRank = floor === undefined ? 0 : RANK[floor];
  const after = afterOf(query.get("cursor"));
  const limit = Math.min(MAX_LIMIT, Math.max(1, Number(query.get("limit") ?? DEFAULT_LIMIT) || DEFAULT_LIMIT));
  const matching = all
    .filter((f) => (swimlane === null || f.swimlane === swimlane) && (kinds.length === 0 || kinds.includes(f.kind)) && RANK[f.severity] >= minRank && (after === null || f.id < after))
    .sort((a, b) => b.id - a.id);
  const items = matching.slice(0, limit);
  const last = items[items.length - 1];
  return { items, next: matching.length > limit && last !== undefined ? cursorAfter(last.id) : null };
}
