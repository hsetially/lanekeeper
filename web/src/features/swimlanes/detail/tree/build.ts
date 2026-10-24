import { DriftState, type FileClass, type FileEntry, type PickupState } from "@/api/generated/model";
import { worstPickup } from "@/components/pickup";

// The files of a swimlane as a tree, built from the folders the person has opened. The server lists ONE folder at a time (a
// keyset page of `FileEntry`), so a folder that has not been opened has no children yet (`lazy`), and the tree only ever holds
// what was asked for. This is pure data: the component draws it. Nothing here decides a fact about a file: the drift state is the
// server's, and "uses the tenant file" is read from the `logical_file` and `tenant` the registry gave each entry (CG2); a name is
// never cut at a dash on this side.

/** The id of a row that loads the next page of a folder: this, then the folder. */
export const MORE_PREFIX = "\u0000more:";

export type Folders = ReadonlyMap<string, { readonly entries: readonly FileEntry[]; readonly hasMore: boolean }>;

export type DataNode = {
  /** The path of a file or folder (no trailing slash); `MORE_PREFIX` and the folder for a Load more row. */
  readonly id: string;
  readonly name: string;
  readonly isDir: boolean;
  /** A folder whose entries have not been loaded. */
  readonly lazy: boolean;
  readonly drift: DriftState;
  /** The most urgent pickup state the hub stated for the files this row stands for (a base file and its tenant files); null when it stated none. */
  readonly pickup: PickupState | null;
  readonly entry: FileEntry | null;
  /** The tenant files a logical row stands for (empty when it is one file). */
  readonly usesTenantFile: readonly string[];
  readonly children: DataNode[];
  /** For a Load more row: the folder it loads. Null for every other row. */
  readonly more: string | null;
};

export type BuildOptions = {
  /** Merge a base file and the tenant files that replace it into one row, by `logical_file`. */
  readonly logical: boolean;
  /** Text to find in names, in any case; "" for none. Plain text, never a pattern. */
  readonly filter: string;
  readonly fileClass: FileClass | null;
};

const URGENCY: readonly DriftState[] = [DriftState.conflict, DriftState.nfs_ahead, DriftState.git_ahead, DriftState.unknown, DriftState.untracked, DriftState.intentional_divergence, DriftState.in_sync];
const urgency = (state: DriftState): number => URGENCY.indexOf(state);

const trimSlash = (path: string): string => path.replace(/\/+$/, "");
const baseName = (path: string): string => path.slice(path.lastIndexOf("/") + 1);

function more(prefix: string): DataNode {
  return { id: `${MORE_PREFIX}${prefix}`, name: "Load more", isDir: false, lazy: false, drift: DriftState.in_sync, pickup: null, entry: null, usesTenantFile: [], children: [], more: prefix };
}

function merged(entries: readonly FileEntry[]): FileEntry[][] {
  const groups = new Map<string, FileEntry[]>();
  for (const entry of entries) {
    const key = entry.is_dir ? `dir:${entry.path}` : `file:${entry.logical_file ?? entry.path}`;
    const group = groups.get(key);
    if (group) group.push(entry);
    else groups.set(key, [entry]);
  }
  return [...groups.values()];
}

function folderNodes(prefix: string, folders: Folders, options: BuildOptions, depth: number): DataNode[] {
  const loaded = folders.get(prefix);
  if (!loaded) return [];
  const groups = options.logical ? merged(loaded.entries) : loaded.entries.map((entry) => [entry]);
  const nodes: DataNode[] = [];
  for (const group of groups) {
    const base = group.find((e) => e.tenant == null) ?? group[0];
    if (base === undefined) continue;
    const tenantFiles = group.filter((e) => e.tenant != null).map((e) => e.path);
    if (!base.is_dir && options.fileClass !== null && base.class !== options.fileClass) continue;
    const id = trimSlash(base.path);
    const name = base.is_dir ? baseName(id) : options.logical && group.length > 0 ? baseName(base.logical_file ?? id) : baseName(id);
    const needle = options.filter.trim().toLowerCase();
    const own = needle !== "" && (name.toLowerCase().includes(needle) || id.toLowerCase().includes(needle));
    // A folder whose own name matches keeps everything loaded under it; otherwise only what matches is kept.
    const below: BuildOptions = own ? { ...options, filter: "" } : options;
    const children = base.is_dir && depth < MAX_DEPTH ? folderNodes(id, folders, below, depth + 1) : [];
    if (needle !== "" && !own && children.length === 0) continue;
    nodes.push({
      id,
      name,
      isDir: base.is_dir,
      lazy: base.is_dir && !folders.has(id),
      drift: group.reduce<DriftState>((worst, e) => (urgency(e.drift) < urgency(worst) ? e.drift : worst), group[0]?.drift ?? DriftState.in_sync),
      pickup: worstPickup(group.map((e) => e.pickup_state)),
      entry: base,
      usesTenantFile: options.logical ? tenantFiles : [],
      children,
      more: null,
    });
  }
  nodes.sort((a, b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name));
  if (loaded.hasMore) nodes.push(more(prefix));
  return nodes;
}

/** A path is at most 4,096 characters, so a tree is never deeper than that; the walk stops far sooner. */
const MAX_DEPTH = 64;

export function buildTree(folders: Folders, options: BuildOptions): DataNode[] {
  return folderNodes("", folders, options, 0);
}
