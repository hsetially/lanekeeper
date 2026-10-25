// The reasons a line of the design's States specimens is not in the app (tier 2, see matrix.ts `DesignStates`). Each is a row of design/DEVIATIONS.md,
// named by its first column, or a contract that cannot fill it; "candidate row" is a difference nobody has written a row for yet, for the design
// owner to approve or the app to fix. The list of differences of a state is compared with what is missing both ways, so a reason cannot outlive its
// difference and a difference cannot appear without one.
import type { Difference } from "./parity";

export const WHY = {
  times: "Times: absolute times are UTC with the zone spelled out, and a relative one is computed against the clock; the design's '10:28 (14 min ago)' has no zone",
  permission: "Permission state: 'Ask an Admin' instead of naming the Admin (CG14: a non-admin cannot be told who the admins are)",
  holder: "Swimlane detail: restart refused with 423 (State K01): the cause is worked out from the baseline; the holder is not named (CG4b)",
  progress: "Swimlane detail: restart dialog: no restart progress (ServiceState has no restarting state), so there is no 'Watch progress'",
  conflict: "File: 409 dialog: 'View current' shows two texts with no base-to-current diff (CG18), so it is not 'View diff'",
  dataset: "dataset: the scenario's lane and file are not the design's example (sitb for presitc, tx-infinity-core for tx-infinity-core-sit1)",
  paused: "Live indicator and paused banner: the banner says what the connection is doing and what is off in its own words, and the countdown is not announced",
  truncation: "Truncated results banner: 'Showing the first N ...', never 'shown of total' (the API reports only that the result was cut off, CG16)",
  offline: "candidate row: the banner for an offline agent says 'Agent offline since <time>' and what is off; the design's sentence names the lane and the data's age",
  approval: "candidate row: the sent-for-approval dialog says what the person's change needs and shows the request; the design's sentence names the PRESIT swimlane and a count of pending requests",
  token: "candidate row: the Raise PR dialog answers after the request ('Pull request not created') with the token's problem; the design asks for a new token in a dialog of its own",
  empty: "candidate row: a swimlane with no services is told 'No files' in the tree and nothing in the services panel; the design says no services were reported and where to look",
  chooseTwo: "candidate row: the whole-swimlane mode opens with two swimlanes chosen (the first two), so the design's 'Choose two swimlanes to compare' state has no counterpart",
  banner: "candidate row: the Swimlanes list has no banner for an offline agent; its row says it (Agent: 'Offline · 14 min ago', Drift: 'Stale · last known 3 NFS ahead')",
  stays: "Swimlane detail: restart dialog: the refusal stays inside the restart dialog (Cancel and Restart, with its request ID); there is no second dialog with Close",
  pausedRestart: "Live indicator and paused banner: a write is off while updates are paused, with the reason on the button's own tooltip, in the app's words",
} as const;

export const d = (why: keyof typeof WHY, ...texts: string[]): Difference[] => texts.map((text) => ({ text, why: WHY[why] }));
