import { describeRef, isCompareRef, isSwimlaneId } from "../model";

// The two sides of a whole-swimlane comparison, as the screen's three controls (a swimlane on the left; on the right a
// swimlane, a Git branch or a Git tag) and as the references the server compares. Two swimlanes are compared by their
// EFFECTIVE files (the file each one reads, tenant or base), which is also how the server skips identical subtrees; a
// swimlane against Git is compared as it is on NFS.
//
// The address holds the references, so a link from the server (`/compare?left=nfs:sitb&right=git:base@main`) opens as it was
// sent. The controls only make a new pair when the person changes one.

export type Target = "lane" | "branch" | "tag";
export type Pair = { left: string | null; right: string | null };

export function sidesOf(pair: Pair): { leftLane: string | null; target: Target; rightName: string | null } {
  const left = pair.left !== null && isCompareRef(pair.left) ? describeRef(pair.left) : null;
  const right = pair.right !== null && isCompareRef(pair.right) ? describeRef(pair.right) : null;
  const target: Target = right?.kind === "git" ? (right.repo === "tenant" ? "branch" : "tag") : "lane";
  return { leftLane: left?.kind === "swimlane" ? left.name : null, target, rightName: right?.name ?? null };
}

const leftRef = (target: Target, lane: string): string => (target === "lane" ? `effective:${lane}` : `nfs:${lane}`);

function rightRef(target: Target, name: string): string | null {
  if (target === "lane") return isSwimlaneId(name) ? `effective:${name}` : null;
  const ref = `git:${target === "branch" ? "tenant" : "base"}@${name}`;
  return isCompareRef(ref) ? ref : null;
}

/** A new left swimlane; the right side keeps its kind, and both sides stay the pair a person would have made. */
export function withLeftLane(pair: Pair, lane: string): Pair {
  if (!isSwimlaneId(lane)) return pair;
  const { target, rightName } = sidesOf(pair);
  return { left: leftRef(target, lane), right: rightName === null ? null : rightRef(target, rightName) };
}

/** Another kind of right side: the name of the old kind means nothing in the new one, so the right side is empty again. */
export function withTarget(pair: Pair, target: Target): Pair {
  const { leftLane } = sidesOf(pair);
  return { left: leftLane === null ? null : leftRef(target, leftLane), right: null };
}

export function withRight(pair: Pair, name: string, target: Target = sidesOf(pair).target): Pair {
  return { left: pair.left, right: rightRef(target, name) ?? pair.right };
}

/** The pair the other way round, for two swimlanes. A swimlane and Git are not the same kind of thing, so they do not swap. */
export function swapped(pair: Pair): Pair | null {
  const { leftLane, target, rightName } = sidesOf(pair);
  if (target !== "lane" || leftLane === null || rightName === null) return null;
  return { left: leftRef("lane", rightName), right: rightRef("lane", leftLane) };
}
