import { useCallback, useId, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent, type ReactNode } from "react";
import { ChevronDown, ChevronRight } from "@/icons";
import { cn } from "@/lib/utils";
import { VirtualList, type VirtualListHandle } from "./VirtualList";

// The handoff's TreeView: `role=tree`, rows of 28 px, 16 px of indent a level, a chevron, a name and whatever the screen puts
// at the end of the row (a status badge). The tree is controlled: the screen owns which folders are open and fetches the
// children of a folder that is opened for the first time (`lazy`), because the server lists one folder at a time with a cursor.
//
// The keyboard follows the ARIA tree pattern with `aria-activedescendant` on the tree: Down and Up move, Right opens a folder
// and then enters it, Left closes it and then goes to its parent, Home and End jump, Enter selects. Rows are virtualised.

export type TreeNode = {
  id: string;
  /** What is shown. Text from a file name is data: wrap it in SafeText. */
  label: ReactNode;
  children?: readonly TreeNode[];
  /** A folder whose children are not loaded yet: it can open, and the screen is told to fetch them. */
  lazy?: boolean;
  icon?: ReactNode;
  /** The end of the row. */
  trailing?: ReactNode;
};

export type FlatNode = {
  node: TreeNode;
  level: number;
  posInSet: number;
  setSize: number;
  expandable: boolean;
  expanded: boolean;
  parentId: string | null;
};

/** The rows that are visible: a depth-first walk that does not enter a folder that is closed. No recursion. */
export function flattenTree(nodes: readonly TreeNode[], expanded: ReadonlySet<string>): FlatNode[] {
  const out: FlatNode[] = [];
  type Frame = { siblings: readonly TreeNode[]; index: number; level: number; parentId: string | null };
  const stack: Frame[] = [{ siblings: nodes, index: 0, level: 1, parentId: null }];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (!frame) break;
    const node = frame.siblings[frame.index];
    if (node === undefined) {
      stack.pop();
      continue;
    }
    frame.index += 1;
    const children = node.children ?? [];
    const expandable = node.lazy === true || children.length > 0;
    const open = expandable && expanded.has(node.id);
    out.push({ node, level: frame.level, posInSet: frame.index, setSize: frame.siblings.length, expandable, expanded: open, parentId: frame.parentId });
    if (open && children.length > 0) stack.push({ siblings: children, index: 0, level: frame.level + 1, parentId: node.id });
  }
  return out;
}

const ROW_HEIGHT = 28;
const INDENT = 16;

type TreeViewProps = Omit<HTMLAttributes<HTMLDivElement>, "children" | "role" | "aria-label" | "onSelect"> & {
  label: string;
  nodes: readonly TreeNode[];
  expanded: ReadonlySet<string>;
  /** The new set of open folders, and the folder that changed. */
  onExpandedChange: (next: Set<string>, changedId: string) => void;
  selectedId?: string | null;
  onSelect?: (id: string) => void;
};

export function TreeView({ label, nodes, expanded, onExpandedChange, selectedId = null, onSelect, className, ...rest }: TreeViewProps) {
  const treeId = useId();
  const listRef = useRef<VirtualListHandle>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const flat = useMemo(() => flattenTree(nodes, expanded), [nodes, expanded]);
  const indexById = useMemo(() => new Map(flat.map((f, i) => [f.node.id, i])), [flat]);
  const activeIndex = activeId === null ? null : (indexById.get(activeId) ?? null);

  const domId = useCallback((index: number) => `${treeId}-n-${index}`, [treeId]);

  const setOpen = useCallback(
    (id: string, open: boolean) => {
      const next = new Set(expanded);
      if (open) next.add(id);
      else next.delete(id);
      onExpandedChange(next, id);
    },
    [expanded, onExpandedChange],
  );

  const moveTo = useCallback(
    (index: number) => {
      const target = flat[Math.max(0, Math.min(flat.length - 1, index))];
      if (!target) return;
      setActiveId(target.node.id);
      listRef.current?.scrollToIndex(Math.max(0, Math.min(flat.length - 1, index)), { align: "auto" });
    },
    [flat],
  );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey) return;
    const at = activeIndex ?? -1;
    const current = at >= 0 ? flat[at] : undefined;
    switch (event.key) {
      case "ArrowDown":
        moveTo(at + 1);
        break;
      case "ArrowUp":
        moveTo(at < 0 ? 0 : at - 1);
        break;
      case "Home":
        moveTo(0);
        break;
      case "End":
        moveTo(flat.length - 1);
        break;
      case "ArrowRight":
        if (!current) moveTo(0);
        else if (current.expandable && !current.expanded) setOpen(current.node.id, true);
        else if (current.expanded) moveTo(at + 1);
        break;
      case "ArrowLeft":
        if (!current) break;
        if (current.expanded) setOpen(current.node.id, false);
        else if (current.parentId !== null) moveTo(indexById.get(current.parentId) ?? at);
        break;
      case "Enter":
        if (!current) return;
        onSelect?.(current.node.id);
        break;
      default:
        return;
    }
    event.preventDefault();
  };

  const renderRow = useCallback(
    (index: number) => {
      const item = flat[index];
      if (!item) return null;
      const { node, level, expandable, expanded: open } = item;
      return (
        <div
          style={{ paddingLeft: (level - 1) * INDENT + 8 }}
          className={cn("flex h-full items-center gap-1.5 pr-3 text-sm hover:bg-accent", node.id === activeId && "bg-accent outline-2 -outline-offset-2 outline-ring", node.id === selectedId && "bg-selection")}
        >
          {expandable ? (
            <button
              type="button"
              tabIndex={-1}
              aria-label={`${open ? "Close" : "Open"} ${typeof node.label === "string" ? node.label : node.id}`}
              className="inline-flex size-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground"
              onClick={(event) => {
                // Opening a folder is not choosing it.
                event.stopPropagation();
                setOpen(node.id, !open);
              }}
            >
              {open ? <ChevronDown aria-hidden className="size-3.5" /> : <ChevronRight aria-hidden className="size-3.5" />}
            </button>
          ) : (
            <span aria-hidden className="size-4 shrink-0" />
          )}
          {node.icon ? <span className="shrink-0 text-muted-foreground">{node.icon}</span> : null}
          <span className="min-w-0 flex-1 truncate">{node.label}</span>
          {node.trailing ? <span className="shrink-0">{node.trailing}</span> : null}
        </div>
      );
    },
    [flat, activeId, selectedId, setOpen],
  );

  const getRowProps = useCallback(
    (index: number) => {
      const item = flat[index];
      if (!item) return {};
      return {
        id: domId(index),
        "aria-level": item.level,
        "aria-posinset": item.posInSet,
        "aria-setsize": item.setSize,
        "aria-selected": item.node.id === selectedId,
        ...(item.expandable ? { "aria-expanded": item.expanded } : {}),
        // The pointer path to what the keyboard does with Enter. It is on the row (the tree item), not on its content.
        onClick: () => {
          setActiveId(item.node.id);
          onSelect?.(item.node.id);
        },
      };
    },
    [flat, domId, selectedId, onSelect],
  );
  const getRowKey = useCallback((index: number) => flat[index]?.node.id ?? index, [flat]);

  return (
    <VirtualList
      {...rest}
      ref={listRef}
      label={label}
      role="tree"
      rowRole="treeitem"
      count={flat.length}
      rowHeight={ROW_HEIGHT}
      renderRow={renderRow}
      getRowProps={getRowProps}
      getRowKey={getRowKey}
      pinnedIndex={activeIndex}
      aria-activedescendant={activeIndex === null ? undefined : domId(activeIndex)}
      onKeyDown={onKeyDown}
      className={cn("focus-visible:outline-offset-[-2px]", className)}
    />
  );
}
