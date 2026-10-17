import { defaultRangeExtractor, useVirtualizer, type Range } from "@tanstack/react-virtual";
import { useCallback, useEffect, useImperativeHandle, useMemo, useReducer, useRef, type HTMLAttributes, type Key, type ReactNode, type Ref } from "react";
import { cn } from "@/lib/utils";

// The one virtualiser of the app (P10): trees, grids, history lists, search results, the code view and the diff all draw
// their rows through this. Only the rows in view (plus an overscan of ten) exist in the DOM, positioned with a CSSOM style
// (React's `style` prop; never a `style=""` attribute, which the hub's CSP refuses).
//
// Rows have a fixed height, a number or one per row (the diff has one per kind), so the scroll position never drifts: there
// is no measuring. The handoff's rules, kept here:
//  - virtualise above 100 rows, render everything below that (`virtualizeAbove`);
//  - `aria-rowcount` on the container is the whole list and `aria-rowindex` on each rendered row is its place in it;
//  - the pinned row (the focused or selected one) stays rendered when it scrolls away, so focus never vanishes.
//
// Fast scrolling is opt-in (`ghostWhenFast`). A window that moves by eight rows or more in one frame cannot be read, and the
// cost of a frame is the rows that entered it, so while the scroll is that fast every row is drawn as a shell (`renderRow` is
// told `ghost`: the row's shape and nothing in it), and once the scroll has been slower than that for 120 ms every row is
// drawn in full. Every row, not only the new ones: the rows that were drawn leave the window in the next frames, and taking
// their content down at once is cheaper than painting and removing it frame by frame. A 5,000-row grid of forty columns
// scrolled end to end in five seconds is the case it is for (P10).

export type VirtualListHandle = {
  scrollToIndex: (index: number, options?: { align?: "start" | "center" | "end" | "auto" }) => void;
  /** The scroll container, for focus and for measuring in a test. */
  element: () => HTMLDivElement | null;
};

type ContainerRole = "list" | "listbox" | "grid" | "table" | "tree" | "treegrid";
type RowRole = "listitem" | "option" | "row" | "treeitem";
export type RowProps = Omit<HTMLAttributes<HTMLDivElement>, "style" | "children" | "role">;

type VirtualListProps = Omit<HTMLAttributes<HTMLDivElement>, "children" | "role" | "aria-label"> & {
  /** The accessible name of the list. */
  label: string;
  count: number;
  /** Pixels, or one per row. A function must be stable between renders unless the heights really change. */
  rowHeight: number | ((index: number) => number);
  /** `ghost` is true for a row that enters at speed when `ghostWhenFast` is on: draw its shell, not its content. */
  renderRow: (index: number, ghost: boolean) => ReactNode;
  role?: ContainerRole;
  rowRole?: RowRole;
  /** Props for the row's own element: an id, `aria-selected`, a handler. */
  getRowProps?: (index: number) => RowProps;
  getRowKey?: (index: number) => Key;
  overscan?: number;
  /** The row that must stay rendered whatever the scroll position. */
  pinnedIndex?: number | null;
  /** A list of at most this many rows is rendered whole. Default 100 (the handoff). */
  virtualizeAbove?: number;
  /** A sticky header that scrolls with the content sideways but not up and down. */
  header?: ReactNode;
  headerHeight?: number;
  /** Width of the content when it is wider than the container (a grid of many columns). */
  minContentWidth?: number | string;
  /** Rows before the first one in `aria-rowindex`, for the header rows of a grid. Default: 1 when there is a header. */
  rowIndexOffset?: number;
  /** Draw rows that enter at speed as shells (see above). For lists of costly rows; default off. */
  ghostWhenFast?: boolean;
  ref?: Ref<VirtualListHandle>;
};

/** Rows the window moves in one frame from which it is "fast". */
const FAST_ROWS = 8;
/** How long the scroll must be slower than that before the rows are drawn in full. */
const SETTLE_MS = 120;

const ROLE_FOR_ROW: Record<ContainerRole, RowRole> = { list: "listitem", listbox: "option", grid: "row", table: "row", tree: "treeitem", treegrid: "row" };

export function VirtualList({
  label,
  count,
  rowHeight,
  renderRow,
  role = "list",
  rowRole,
  getRowProps,
  getRowKey,
  overscan = 10,
  pinnedIndex = null,
  virtualizeAbove = 100,
  header,
  headerHeight = 0,
  minContentWidth,
  rowIndexOffset,
  ghostWhenFast = false,
  ref,
  className,
  ...rest
}: VirtualListProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const estimateSize = useMemo(() => (typeof rowHeight === "number" ? () => rowHeight : rowHeight), [rowHeight]);
  const headerRows = header === undefined ? 0 : 1;
  const offset = rowIndexOffset ?? headerRows;

  const rangeExtractor = useCallback(
    (range: Range): number[] => {
      if (count <= virtualizeAbove) return Array.from({ length: count }, (_, i) => i);
      const indexes = defaultRangeExtractor(range);
      if (pinnedIndex !== null && pinnedIndex >= 0 && pinnedIndex < count && !indexes.includes(pinnedIndex)) {
        if (indexes.length === 0 || pinnedIndex < (indexes[0] ?? 0)) indexes.unshift(pinnedIndex);
        else indexes.push(pinnedIndex);
      }
      return indexes;
    },
    [count, virtualizeAbove, pinnedIndex],
  );

  // Fast scrolling: where the window was when the last render reached the screen, and until when the rows are shells. The
  // adapter calls `onChange` after it has re-rendered, so the speed is read in the render itself, from where the window was,
  // and what must be remembered is only written once the render is committed.
  const committedStart = useRef(0);
  const settleAt = useRef(0);
  const settleTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [, settle] = useReducer((n: number) => n + 1, 0);
  useEffect(() => () => clearTimeout(settleTimer.current), []);

  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize,
    overscan,
    rangeExtractor,
    scrollMargin: headerHeight,
    ...(getRowKey ? { getItemKey: getRowKey } : {}),
  });

  // The cache of sizes is only rebuilt when the count changes; a new function of heights must reset it by hand.
  const { measure } = virtualizer;
  useEffect(() => {
    measure();
  }, [estimateSize, measure]);

  useImperativeHandle(
    ref,
    () => ({
      scrollToIndex: (index, options) => virtualizer.scrollToIndex(index, options),
      element: () => scrollRef.current,
    }),
    [virtualizer],
  );

  const containerRole = role;
  const itemRole = rowRole ?? ROLE_FOR_ROW[containerRole];
  const tabular = containerRole === "grid" || containerRole === "table" || containerRole === "treegrid";
  const rowIsRow = itemRole === "row";
  const items = virtualizer.getVirtualItems();
  const margin = virtualizer.options.scrollMargin;
  const start = virtualizer.range?.startIndex ?? 0;
  const fastNow = ghostWhenFast && count > virtualizeAbove && Math.abs(start - committedStart.current) >= FAST_ROWS;
  if (fastNow) settleAt.current = performance.now() + SETTLE_MS;
  const quick = ghostWhenFast && performance.now() < settleAt.current;
  // After this render is on screen: where the window is, and the timer that ends the shells.
  useEffect(() => {
    committedStart.current = start;
    if (fastNow) {
      clearTimeout(settleTimer.current);
      settleTimer.current = setTimeout(settle, SETTLE_MS + 20);
    }
  });

  return (
    <div
      {...rest}
      ref={scrollRef}
      role={containerRole}
      aria-label={label}
      {...(tabular ? { "aria-rowcount": count + headerRows } : {})}
      tabIndex={rest.tabIndex ?? 0}
      data-slot="virtual-list"
      className={cn("relative min-h-0 overflow-auto overscroll-contain", className)}
    >
      {header !== undefined ? (
        <div data-slot="virtual-header" className="sticky top-0 z-10 bg-background" style={{ height: headerHeight, minWidth: minContentWidth }}>
          {header}
        </div>
      ) : null}
      <div role="presentation" data-slot="virtual-spacer" className="relative w-full" style={{ height: virtualizer.getTotalSize(), minWidth: minContentWidth }}>
        {items.map((item) => {
          const { className: rowClass, ...rowProps } = getRowProps?.(item.index) ?? {};
          const ghost = quick;
          return (
            <div
              key={item.key}
              // A row of a table says where it is in the whole table; an item of a list says where it is in the whole set.
              {...(rowIsRow ? { "aria-rowindex": item.index + 1 + offset } : { "aria-posinset": item.index + 1, "aria-setsize": count })}
              {...rowProps}
              {...(ghost ? { "aria-hidden": true } : {})}
              role={itemRole}
              data-slot="virtual-row"
              data-index={item.index}
              className={cn("absolute left-0 top-0 w-full", rowClass)}
              style={{ height: item.size, transform: `translateY(${item.start - margin}px)` }}
            >
              {renderRow(item.index, ghost)}
            </div>
          );
        })}
      </div>
    </div>
  );
}
