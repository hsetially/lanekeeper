import type { CompareState, TreeCompareEntry } from "@/api/generated/model";

// The files that differ between two sides, as the folder tree of the design ("Differing directories"). The server sends a
// flat list of logical files and their states; the folders are made here from the paths, for display only. A file that is the
// same on both sides is not shown (a folder is listed because something in it differs).

export type FileNode = { kind: "file"; id: string; path: string; name: string; state: Exclude<CompareState, "same">; count: 1 };
export type DirNode = { kind: "dir"; id: string; path: string; name: string; count: number; children: Array<DirNode | FileNode> };
export type TreeNodeData = DirNode | FileNode;

export type Filter = "all" | "different" | "left_only" | "right_only";

export function filterEntries(entries: readonly TreeCompareEntry[], filter: Filter): TreeCompareEntry[] {
  return entries.filter((entry) => entry.state !== "same" && (filter === "all" || entry.state === filter));
}

export function countStates(entries: readonly TreeCompareEntry[]): Record<CompareState, number> {
  const counts: Record<CompareState, number> = { different: 0, left_only: 0, right_only: 0, same: 0 };
  for (const entry of entries) counts[entry.state] += 1;
  return counts;
}

const byName = (a: TreeNodeData, b: TreeNodeData): number => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "dir" ? -1 : 1);

/** Folders first, each level in name order. No recursion: a path is walked by a loop, and the sort is done level by level. */
export function buildTree(entries: readonly TreeCompareEntry[]): TreeNodeData[] {
  const roots: Array<DirNode | FileNode> = [];
  const dirs = new Map<string, DirNode>();
  for (const entry of entries) {
    if (entry.state === "same") continue;
    const parts = entry.file.split("/");
    const name = parts.pop() ?? entry.file;
    let siblings = roots;
    let path = "";
    for (const part of parts) {
      path = path === "" ? part : `${path}/${part}`;
      let dir = dirs.get(path);
      if (dir === undefined) {
        dir = { kind: "dir", id: `d:${path}`, path, name: part, count: 0, children: [] };
        dirs.set(path, dir);
        siblings.push(dir);
      }
      dir.count += 1;
      siblings = dir.children;
    }
    siblings.push({ kind: "file", id: `f:${entry.file}`, path: entry.file, name, state: entry.state, count: 1 });
  }
  const stack: Array<Array<DirNode | FileNode>> = [roots];
  while (stack.length > 0) {
    const level = stack.pop();
    if (level === undefined) break;
    level.sort(byName);
    for (const node of level) if (node.kind === "dir") stack.push(node.children);
  }
  return roots;
}

/** Every id in the tree, depth first, folders before the files they hold. For tests and for "expand all". */
export function flattenIds(nodes: readonly TreeNodeData[]): string[] {
  const out: string[] = [];
  const stack: TreeNodeData[] = [...nodes].reverse();
  while (stack.length > 0) {
    const node = stack.pop();
    if (node === undefined) break;
    out.push(node.id);
    if (node.kind === "dir") for (let i = node.children.length - 1; i >= 0; i--) stack.push(node.children[i] as TreeNodeData);
  }
  return out;
}
