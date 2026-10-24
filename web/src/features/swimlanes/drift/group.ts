import { DriftState, type FileEntry } from "@/api/generated/model";
import { sortBySeverity } from "@/components/attribution/severity";

// The Drift tab groups the files that are not in sync by their state. The order of the groups is the order of urgency; inside a
// group the most severe file comes first, then by path, and a file nobody rated comes last. This is layout: the state and the
// severity of each file are the server's.

const ORDER: readonly DriftState[] = [DriftState.conflict, DriftState.nfs_ahead, DriftState.git_ahead, DriftState.unknown, DriftState.untracked, DriftState.intentional_divergence];

export type DriftGroup = { readonly state: DriftState; readonly files: FileEntry[] };

export function groupDrift(entries: readonly FileEntry[]): DriftGroup[] {
  const by = new Map<DriftState, FileEntry[]>();
  for (const entry of entries) {
    if (entry.is_dir || !ORDER.includes(entry.drift)) continue;
    const files = by.get(entry.drift);
    if (files) files.push(entry);
    else by.set(entry.drift, [entry]);
  }
  return ORDER.filter((state) => by.has(state)).map((state) => ({
    state,
    files: sortBySeverity(by.get(state) ?? [], (file) => file.severity, (a, b) => a.path.localeCompare(b.path)),
  }));
}

export type DriftRow = { readonly kind: "group"; readonly state: DriftState; readonly count: number; readonly open: boolean } | { readonly kind: "file"; readonly file: FileEntry; readonly state: DriftState };

/** The rows of the list: a header for each group, then its files unless the group is closed. */
export function rowsOf(groups: readonly DriftGroup[], closed: ReadonlySet<DriftState>): DriftRow[] {
  return groups.flatMap((group): DriftRow[] => {
    const open = !closed.has(group.state);
    const header: DriftRow = { kind: "group", state: group.state, count: group.files.length, open };
    return open ? [header, ...group.files.map((file): DriftRow => ({ kind: "file", file, state: group.state }))] : [header];
  });
}
