// What a missing baseline of the app against itself means (the plan's "Baseline bootstrap"). Baselines are made in CI, by the
// `visual-baselines` job, and reviewed there; a run anywhere else must not write PNGs into the tree and must not read as a pass.
//
//   LK_UPDATE_BASELINES set  Playwright creates the missing ones (`updateSnapshots: "missing"`, playwright.config.ts) and fails the
//                            test that needed one, as it does for a first snapshot; the next run compares.
//   otherwise                Playwright is told to create nothing (`updateSnapshots: "none"`), so a missing baseline is a FAILURE where
//                            CI runs and NOT RUN, said in an annotation, anywhere else.
// `--update-snapshots=all` (a person refreshing the baselines on purpose) takes the comparison as it is: it writes them.

/**
 * @param {{ exists: boolean; ci: boolean; updateSnapshots: string }} state
 * @returns {boolean} true when the comparison is skipped as NOT RUN
 */
export function baselineIsNotRun({ exists, ci, updateSnapshots }) {
  return !exists && !ci && updateSnapshots === "none";
}
