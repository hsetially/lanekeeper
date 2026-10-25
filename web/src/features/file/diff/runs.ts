import type { DiffHunk } from "@/api/generated/model";

/**
 * How many runs of changed lines the hunks hold: the changes `n` and `p` walk in the diff, counted without laying the rows out.
 * A run is a stretch of removed and added lines with no context line between; a new hunk starts a new run.
 */
export function countChangeRuns(hunks: readonly DiffHunk[]): number {
  let runs = 0;
  for (const hunk of hunks) {
    let inRun = false;
    for (const line of hunk.lines) {
      const changed = line.kind !== "context";
      if (changed && !inRun) runs += 1;
      inRun = changed;
    }
  }
  return runs;
}
