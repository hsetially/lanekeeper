// Which two sides a file's Diff tab compares (File, the ref picker). The `diff` search value names the LEFT side; the right is
// this swimlane's NFS, as the design draws it ("Right: NFS (current)"), unless the person swaps the sides. The sides go to the
// server as typed references (`CompareRef`); the engine computes the diff and the browser never reads a side's file to compare.

export type Side =
  | { kind: "git" }
  | { kind: "baseline"; lane: string }
  | { kind: "lane"; id: string }
  | { kind: "tag"; name: string }
  | { kind: "branch"; name: string };

/** What the sides depend on: the swimlane, the file's tenant (null for a base file) and the swimlane's base version. */
export type SideContext = {
  lane: string;
  tenant: string | null;
  baseCommit: string | null;
  /** "v2.3.1 + 4": the base version as the design writes it. */
  baseLabel: string;
};

const LANE = /^[a-z0-9][a-z0-9-]{0,62}$/;
const GIT_NAME = /^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$/;
const COMPARE_REF = /^((nfs|baseline|effective):[a-z0-9][a-z0-9-]{0,62}|git:(base|tenant)@.{1,255})$/;

export function parseDiffParam(value: string | undefined): Side {
  if (value === undefined || value === "git") return { kind: "git" };
  const tag = /^tag:(.*)$/.exec(value)?.[1];
  if (tag !== undefined) return GIT_NAME.test(tag) && !tag.includes("..") ? { kind: "tag", name: tag } : { kind: "git" };
  const branch = /^branch:(.*)$/.exec(value)?.[1];
  if (branch !== undefined) return GIT_NAME.test(branch) && !branch.includes("..") ? { kind: "branch", name: branch } : { kind: "git" };
  const baseline = /^baseline:(.*)$/.exec(value)?.[1];
  if (baseline !== undefined) return LANE.test(baseline) ? { kind: "baseline", lane: baseline } : { kind: "git" };
  return LANE.test(value) ? { kind: "lane", id: value } : { kind: "git" };
}

/** The `diff` search value for a side. `git` is the default and is left out of the URL by the caller. */
export function diffParamOf(side: Side): string {
  switch (side.kind) {
    case "git":
      return "git";
    case "baseline":
      return `baseline:${side.lane}`;
    case "lane":
      return side.id;
    case "tag":
      return `tag:${side.name}`;
    case "branch":
      return `branch:${side.name}`;
  }
}

function refOf(side: Side, context: SideContext): string | null {
  const ref = ((): string | null => {
    switch (side.kind) {
      case "git":
        return context.tenant !== null ? `git:tenant@${context.tenant}` : context.baseCommit !== null ? `git:base@${context.baseCommit}` : null;
      case "baseline":
        return `baseline:${side.lane}`;
      case "lane":
        return `nfs:${side.id}`;
      case "tag":
        return `git:base@${side.name}`;
      case "branch":
        return `git:${context.tenant !== null ? "tenant" : "base"}@${side.name}`;
    }
  })();
  return ref !== null && COMPARE_REF.test(ref) ? ref : null;
}

/** The two references to send, or null when a side cannot be named (nothing is sent then). */
export function compareSides(side: Side, swapped: boolean, context: SideContext): { left: string; right: string } | null {
  const chosen = refOf(side, context);
  const nfs = `nfs:${context.lane}`;
  if (chosen === null || !COMPARE_REF.test(nfs)) return null;
  return swapped ? { left: nfs, right: chosen } : { left: chosen, right: nfs };
}

/** What a side is called on its button. */
export function sideLabel(side: Side, context: SideContext): string {
  switch (side.kind) {
    case "git":
      return context.tenant !== null ? `Git (${context.tenant} head)` : `Git (${context.baseLabel})`;
    case "baseline":
      return "Baseline";
    case "lane":
      return side.id;
    case "tag":
      return side.name;
    case "branch":
      return side.name;
  }
}
