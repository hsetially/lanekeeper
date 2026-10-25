import type { SettingChange } from "@/api/generated/model";

// The settings behind a diff, in the words of the design (Diff, 1a): "2 settings changed, 1 added", one chip per setting with the
// old value struck against the new, and the list of changes with their lines. The engine computed the changes; this only words
// them. The values are text from the file (already masked by the server where the secret scan flagged them): the caller draws them as data.

const MAX_CHIPS = 12;
const MAX_LIST = 200;

export type Chip = { prefix: string; key: string; kind: "changed" | "added" | "removed" | "type changed"; from: string | null; to: string | null };
export type ListItem = { key: string; kind: "Changed" | "Added" | "Removed"; line: number | null };
export type Summary = { headline: string; total: number; chips: Chip[]; hiddenChips: number; list: ListItem[]; hiddenList: number };

const plural = (n: number): string => `${n} ${n === 1 ? "setting" : "settings"}`;

function chipOf(change: SettingChange): Chip {
  const at = change.path.lastIndexOf(".");
  return {
    prefix: at < 0 ? "" : change.path.slice(0, at + 1),
    key: at < 0 ? change.path : change.path.slice(at + 1),
    kind: change.kind === "type_changed" ? "type changed" : change.kind,
    from: change.left?.text ?? null,
    to: change.right?.text ?? null,
  };
}

export function summarise(changes: readonly SettingChange[]): Summary {
  const changed = changes.filter((c) => c.kind === "changed" || c.kind === "type_changed").length;
  const added = changes.filter((c) => c.kind === "added").length;
  const removed = changes.filter((c) => c.kind === "removed").length;
  const parts: string[] = [];
  if (changed > 0) parts.push(`${plural(changed)} changed`);
  if (added > 0) parts.push(parts.length > 0 ? `${added} added` : `${plural(added)} added`);
  if (removed > 0) parts.push(parts.length > 0 ? `${removed} removed` : `${plural(removed)} removed`);
  return {
    headline: parts.join(", "),
    total: changes.length,
    chips: changes.slice(0, MAX_CHIPS).map(chipOf),
    hiddenChips: Math.max(0, changes.length - MAX_CHIPS),
    list: changes.slice(0, MAX_LIST).map((c) => ({
      key: c.path,
      kind: c.kind === "added" ? "Added" : c.kind === "removed" ? "Removed" : "Changed",
      line: (c.right_location ?? c.left_location)?.lines.start ?? null,
    })),
    hiddenList: Math.max(0, changes.length - MAX_LIST),
  };
}
