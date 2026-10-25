import type { ReactElement } from "react";
import { PickupChip } from "@/components/pickup";
import { folderChip, type PickupNodeLike, type PickupRollup } from "./rollup";

// The pickup mark at the end of a row of the file tree. A file shows the state the hub gave for it when something is waiting (an
// icon, named for a screen reader and explained in the tooltip), as the tree marks drift: live files, and files the hub gave no
// state for, are not marked. A folder shows what its LOADED files say (see rollup.ts) and is never called live. A "Load more" row
// has no mark.

export function TreePickup({ node, rollup }: { node: PickupNodeLike; rollup: PickupRollup | undefined }): ReactElement | null {
  if (node.more !== null) return null;
  if (!node.isDir) return <PickupChip state={node.pickup ?? node.entry?.pickup_state} subject="file" density="compact" hideLive />;
  const chip = rollup === undefined ? null : folderChip(rollup);
  if (chip === null) return null;
  return <PickupChip state={chip.state} subject="file" density="compact" words={chip.words} {...(chip.count === null ? {} : { count: chip.count })} />;
}
