import type { DriftState, PrLink } from "@/api/generated/model";

// "PR open" and "Fix merged, awaiting sync" (Consequence dialogs, frame 1l), derived from a swimlane's pull-request links and
// joined on the file's path. The pure part: the data hook is use-pr-badges.ts.
//
//  - An OPEN link is the "PR open" badge. If the file has changed since the link was made (`nfs_hash` is not the file's hash now),
//    the badge is still shown, because the pull request is still open on that file, and says it was raised for an earlier version.
//  - A MERGED link is "Fix merged, awaiting sync" only for a file that still differs (a drift state that asks a person to look)
//    and, when the file's hash is known, only if it is the content that was merged. The server stores no such label
//    (crates/hub-writes/src/prs/links.rs, plan A17: "whoever shows the drift derives it from the link's state at read time"), so
//    a merged link on a file that is in sync, or that has changed again, says nothing.
//  - A closed link carries nothing.

export type PrBadgeKind = "open" | "merged";
export type PrBadgeInfo = {
  readonly kind: PrBadgeKind;
  readonly link: PrLink;
  /** An open link raised for another version of the file than the one it has now. */
  readonly outdated: boolean;
};

/** What the caller knows about the file. `hash` and `drift` are the file's now (`FileEntry.hash`, `FileEntry.drift`). */
export type PrFacts = {
  readonly path: string;
  readonly hash?: string | null | undefined;
  readonly drift?: DriftState | null | undefined;
  /** The file differs, when the caller knows that and not the state (a finding of the drift check). A `drift` state, if given, decides instead. */
  readonly differs?: boolean | undefined;
};

/** The links of one swimlane by path, newest first. */
export type PrIndex = ReadonlyMap<string, readonly PrLink[]>;

/** Keeps the links of `swimlane` and groups them by path, newest (highest id) first. */
export function buildPrIndex(swimlane: string, links: readonly PrLink[]): PrIndex {
  const byPath = new Map<string, PrLink[]>();
  for (const link of links) {
    if (link.swimlane !== swimlane) continue;
    const list = byPath.get(link.path);
    if (list === undefined) byPath.set(link.path, [link]);
    else list.push(link);
  }
  for (const list of byPath.values()) list.sort((a, b) => b.id - a.id);
  return byPath;
}

/** A drift state that asks a person to look: not in sync, and not one a person decided to keep. */
function differs(facts: PrFacts): boolean {
  const { drift } = facts;
  if (drift === undefined || drift === null) return facts.differs === true;
  return drift !== "in_sync" && drift !== "intentional_divergence";
}

export function prBadgeFor(index: PrIndex, facts: PrFacts): PrBadgeInfo | null {
  const links = index.get(facts.path);
  if (links === undefined) return null;
  const hash = facts.hash ?? null;

  const open = links.filter((l) => l.state === "open");
  const open0 = (hash === null ? undefined : open.find((l) => l.nfs_hash === hash)) ?? open[0];
  if (open0 !== undefined) return { kind: "open", link: open0, outdated: hash !== null && open0.nfs_hash !== hash };

  if (differs(facts)) {
    const merged = links.find((l) => l.state === "merged" && (hash === null || l.nfs_hash === hash));
    if (merged !== undefined) return { kind: "merged", link: merged, outdated: false };
  }
  return null;
}
