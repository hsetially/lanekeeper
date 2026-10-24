import { useCallback, useId, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent, type ReactNode } from "react";
import { ChevronDown, ChevronRight } from "@/icons";
import { cn } from "@/lib/utils";
import { VirtualList, type VirtualListHandle } from "./VirtualList";

// The handoff's TreeView: `role=tree`, rows of 28 px, 16 px of indent a level, a chevron, a name and whatever the screen puts
// at the end of the row (a status badge). The tree is controlled: the screen owns which folders are open and fetches the
// children of a folder that is opened for the first time (`lazy`), because the server lists one folder at a time with a cursor.
//
// The keyboard follows the ARIA tree pattern with `aria-activedescendant` on the tree: Down and Up move, Right opens a folder
// and then enters it, Left closes it and then goes to its parent, Home and End jump, Enter selects, and typing jumps to the next
// row that starts with what was typed (the keymap's "Tree only: type-ahead"). Rows are virtualised.

export type TreeNode = {
  id: string;
  /** What is shown. Text from a file name is data: wrap it in SafeText. */
  label: ReactNode;
  /**
   * What type-ahead matches a typed prefix against, when the label is not plain text: a name without its decoration. Without it
   * the label is used when it is a string, and the last part of the id otherwise.
   */
  text?: string;
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
/** How long a pause ends what has been typed: the next character starts a new search. */
const TYPEAHEAD_MS = 1000;

/** The text a typed prefix is compared with, lower case. */
function typeaheadText(node: TreeNode): string {
  const own = node.text ?? (typeof node.label === "string" ? node.label : undefined);
  return (own ?? node.id.slice(node.id.lastIndexOf("/") + 1)).toLowerCase();
}

type TreeViewProps = Omit<HTMLAttributes<HTMLDivElement>, "children" | "role" | "aria-label" | "onSelect"> & {
  label: string;
  nodes: readonly TreeNode[];
  expanded: ReadonlySet<string>;
  /** The new set of open folders, and the folder that changed. */
  onExpandedChange: (next: Set<string>, changedId: string) => void;
  selectedId?: string | null;
  onSelect?: (id: string) => void;
  /** Control or Command and Enter on the row under the cursor (the keymap's "Open in new tab"). Without it the browser keeps the key. */
  onOpenInNewTab?: (id: string) => void;
};

export function TreeView({ label, nodes, expanded, onExpandedChange, selectedId = null, onSelect, onOpenInNewTab, className, ...rest }: TreeViewProps) {
  const treeId = useId();
  const listRef = useRef<VirtualListHandle>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const flat = useMemo(() => flattenTree(nodes, expanded), [nodes, expanded]);
  const indexById = useMemo(() => new Map(flat.map((f, i) => [f.node.id, i])), [flat]);
  const activeIndex = activeId === null ? null : (indexById.get(activeId) ?? null);
  const typed = useRef({ text: "", at: 0 });

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

  /** Type-ahead: the row from `from` on (and round to the start) whose text starts with `prefix`, or -1. */
  const findPrefix = (prefix: string, from: number): number => {
    for (let step = 0; step < flat.length; step += 1) {
      const index = (from + step) % flat.length;
      const item = flat[index];
      if (item !== undefined && typeaheadText(item.node).startsWith(prefix)) return index;
    }
    return -1;
  };

  const typeAhead = (character: string, at: number): void => {
    const now = performance.now();
    const char = character.toLowerCase();
    const soFar = now - typed.current.at <= TYPEAHEAD_MS ? typed.current.text : "";
    // The same letter again asks for the next row that starts with it, not for the prefix "bb".
    const repeated = soFar !== "" && soFar === char.repeat(soFar.length);
    // Characters that go on from what was typed look for a row that starts with all of them, from the row the cursor is on.
    let prefix = soFar + char;
    let found = soFar === "" || repeated ? -1 : findPrefix(prefix, Math.max(at, 0));
    if (found < 0) {
      // Anything else is a search of its own for this one character, from the next row on.
      prefix = repeated ? soFar : char;
      found = findPrefix(char, at + 1);
    }
    typed.current = { text: prefix, at: now };
    if (found >= 0) moveTo(found);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || event.altKey) return;
    // Control or Command and Enter is the one chord the tree keeps, when the screen has a new tab to open; any other chord is the
    // browser's or the shell's.
    const chord = event.ctrlKey || event.metaKey;
    if (chord && !(event.key === "Enter" && onOpenInNewTab)) return;
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
        if (chord) onOpenInNewTab?.(current.node.id);
        else onSelect?.(current.node.id);
        break;
      default:
        // A printable character is type-ahead (a space only inside what is being typed: it is not a row's key here).
        if (event.key.length !== 1 || (event.key === " " && typed.current.text === "")) return;
        typeAhead(event.key, at);
        break;
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
