import { PickupState } from "@/api/generated/model";
import { PICKUP_ORDER, isPickupState, worstPickup } from "@/components/pickup";

// A folder's pickup state in the file tree, worked out from what the tree has LOADED and nothing else. The hub lists one folder
// at a time, so a folder's files may be only partly in hand: a folder that was never opened has none, and a long one has a "Load
// more" row. So the rollup is a count of the loaded files with a stated pickup state, and it says whether it is complete:
//  - "N of the M files loaded in this folder need a notify or a restart" is true however much is not loaded;
//  - a folder is never called live (that would claim every file, including the ones not loaded, and the ones the hub gave no
//    state for), so a folder whose files are all live gets no chip;
//  - a file the hub gave no state for (`pickup_state` is null: the hub does not fill it in yet) is "not stated", not "live".
// The folder's own `pickup_state` counts too, when the hub states one (a folder first seen after the config-server started is
// exactly the case that needs a config-server restart).
//
// The input is the tree the file view builds (`DataNode` of features/swimlanes/detail/tree/build.ts), described by the few
// fields the rollup reads so that the rollup does not depend on the tree.

export type PickupNodeLike = {
  readonly id: string;
  readonly isDir: boolean;
  /** A folder whose entries have not been loaded. */
  readonly lazy: boolean;
  /** For a "Load more" row: the folder it loads. Null for every other row. */
  readonly more: string | null;
  readonly entry: { readonly pickup_state: PickupState | null } | null;
  /** The worst state of the files a row stands for, when the tree merges a base file and its tenant files into one row. */
  readonly pickup?: PickupState | null | undefined;
  readonly children: readonly PickupNodeLike[];
};

export type PickupRollup = {
  /** The loaded files below the folder (at any depth) by stated state, most urgent first, no zero rows. */
  readonly counts: ReadonlyArray<readonly [PickupState, number]>;
  /** The folder's own state, when the hub stated one. */
  readonly own: PickupState | null;
  /** The most urgent state of the folder itself and of its loaded files; null when nothing was stated. */
  readonly worst: PickupState | null;
  /** How many files are loaded below the folder, whatever the hub said about them. */
  readonly files: number;
  /** Loaded files the hub gave no pickup state for. */
  readonly unstated: number;
  /** False while something below the folder is not loaded: a folder that was not opened, or a page that was not fetched. */
  readonly complete: boolean;
};

type Acc = { counts: Map<PickupState, number>; files: number; unstated: number; complete: boolean };

const empty = (): Acc => ({ counts: new Map(), files: 0, unstated: 0, complete: true });

function add(into: Acc, from: Acc): void {
  for (const [state, n] of from.counts) into.counts.set(state, (into.counts.get(state) ?? 0) + n);
  into.files += from.files;
  into.unstated += from.unstated;
  into.complete &&= from.complete;
}

function visit(node: PickupNodeLike, out: Map<string, PickupRollup>): Acc {
  const acc = empty();
  if (node.lazy && node.children.length === 0) acc.complete = false;
  for (const child of node.children) {
    if (child.more !== null) {
      acc.complete = false;
    } else if (child.isDir) {
      add(acc, visit(child, out));
    } else {
      acc.files += 1;
      const state = child.pickup ?? child.entry?.pickup_state;
      if (isPickupState(state)) acc.counts.set(state, (acc.counts.get(state) ?? 0) + 1);
      else acc.unstated += 1;
    }
  }
  const ownState = node.entry?.pickup_state;
  const own = isPickupState(ownState) ? ownState : null;
  out.set(node.id, {
    counts: PICKUP_ORDER.flatMap((state): Array<readonly [PickupState, number]> => {
      const n = acc.counts.get(state);
      return n === undefined ? [] : [[state, n]];
    }),
    own,
    worst: worstPickup([own, ...acc.counts.keys()]),
    files: acc.files,
    unstated: acc.unstated,
    complete: acc.complete,
  });
  return acc;
}

/** The rollup of every folder in the tree, by the id of the row, in one pass. Files have none. */
export function pickupRollups(nodes: readonly PickupNodeLike[]): ReadonlyMap<string, PickupRollup> {
  const out = new Map<string, PickupRollup>();
  for (const node of nodes) {
    if (node.more === null && node.isDir) visit(node, out);
  }
  return out;
}

export type FolderChip = {
  readonly state: PickupState;
  /** How many loaded files are in that state; null when the state is the folder's own and no loaded file is in it. */
  readonly count: number | null;
  /** What the chip counted, in words, for the tooltip. */
  readonly words: string;
};

const FILE_VERB: Record<PickupState, readonly [string, string]> = {
  [PickupState.needs_config_server_restart]: ["waits for the config-server to restart.", "wait for the config-server to restart."],
  [PickupState.needs_notify_or_restart]: ["needs a notify or a restart.", "need a notify or a restart."],
  [PickupState.live_within_ttl]: ["is read again by services when their client cache expires.", "are read again by services when their client cache expires."],
  [PickupState.live]: ["is live.", "are live."],
};

const OWN: Record<PickupState, string> = {
  [PickupState.needs_config_server_restart]: "waiting for the config-server to restart.",
  [PickupState.needs_notify_or_restart]: "needing a notify or a restart.",
  [PickupState.live_within_ttl]: "live within the client cache.",
  [PickupState.live]: "live.",
};

/**
 * What to draw on a folder row, or null for nothing. Only the states in which something is waiting get a chip: a folder is never
 * called live, and a folder with no stated state says nothing.
 */
export function folderChip(rollup: PickupRollup): FolderChip | null {
  const state = rollup.worst;
  if (state === null || state === PickupState.live) return null;
  const inState = rollup.counts.find(([s]) => s === state)?.[1] ?? 0;
  if (inState === 0) return { state, count: null, words: `The hub reports the folder itself as ${OWN[state]}` };
  const [one, many] = FILE_VERB[state];
  const parts = [`${inState} of the ${rollup.files} ${rollup.files === 1 ? "file" : "files"} loaded in this folder ${inState === 1 ? one : many}`];
  if (!rollup.complete) parts.push("Files that are not loaded yet are not counted.");
  if (rollup.unstated > 0) parts.push(`The hub gave no pickup state for ${rollup.unstated} of the files.`);
  return { state, count: inState, words: parts.join(" ") };
}
