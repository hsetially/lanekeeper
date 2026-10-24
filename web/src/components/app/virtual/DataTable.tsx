import { useVirtualizer } from "@tanstack/react-virtual";
import { Fragment, memo, useCallback, useEffect, useId, useMemo, useRef, useState, type CSSProperties, type HTMLAttributes, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { ArrowDown, ArrowUp, ChevronsUpDown } from "@/icons";
import { cn } from "@/lib/utils";
import { VirtualList, type VirtualListHandle } from "./VirtualList";

// The handoff's DataTable: a div grid (`role=grid`, `aria-rowcount`, `aria-rowindex`), a sticky header of 32 px, rows of 32 px
// (default) or 28 px (compact), sortable headers with `aria-sort`, a footer of 36 px with "Showing N" and "Load 100 more".
// Rows are virtualised (VirtualList) and only the active row is guaranteed to stay rendered.
//
// A table of many columns (the comparison grid has one for each of up to 64 swimlanes) draws only the columns in view, as it
// draws only the rows in view: the first column stays, the others come and go as the table scrolls sideways, each cell says
// its place among all of them (`aria-colindex`, `aria-colcount`), and the width of the whole table is unchanged. A cell is the
// costly part of a frame of fast scrolling, and forty columns are three times the cells of twelve.
//
// The keyboard cursor is `aria-activedescendant` on the grid, not real focus on a row: rows are created and destroyed as the
// list scrolls, and focus on a row that unmounts would be lost. Arrow keys, Home, End, Page Up and Page Down move it, Enter
// activates, Control or Command and Enter opens in a new tab, Space or x toggles selection (Shift extends the range). After
// "Load more" the cursor lands on the first new row and the grid has focus.
//
// `selectedKeys` and the callbacks are the table's whole contract with a screen: the table never sorts or filters by itself
// (the server does, with a cursor) and never holds data of its own.

export type SortState = { column: string; direction: "asc" | "desc" };

export type Column<Row> = {
  id: string;
  header: ReactNode;
  /** The CSS grid track of the column, for example `minmax(0,1fr)` or `120px`. */
  width: string;
  /** Pixels the column needs at least; the table scrolls sideways when the columns add up to more than the box. */
  minWidth?: number;
  align?: "left" | "right";
  sortable?: boolean;
  cell: (row: Row, index: number) => ReactNode;
};

export type TableFooter = {
  hasMore: boolean;
  onLoadMore: () => void;
  loading: boolean;
  /** The result was cut off at its limit: say so rather than looking complete. */
  truncated?: boolean;
  /** How many rows a click adds, for the label only. Default 100. */
  step?: number;
  /** Known only when the server says so. */
  total?: number;
};

type DataTableProps<Row> = Omit<HTMLAttributes<HTMLDivElement>, "children" | "role" | "aria-label" | "onSelect"> & {
  label: string;
  rows: readonly Row[];
  columns: readonly Column<Row>[];
  rowKey: (row: Row, index: number) => string;
  density?: "default" | "compact";
  sort?: SortState | null;
  onSortChange?: (next: SortState | null) => void;
  selectedKeys?: ReadonlySet<string>;
  /** Enter on the active row, or a double click. */
  onActivate?: (row: Row, index: number) => void;
  /** A click on a row (not on a link or a button in it): opens it. The keyboard cursor moves to the row. */
  onRowClick?: (row: Row, index: number) => void;
  /** Control or Command and Enter on the active row (the keymap's "Open in new tab"). Without it the browser keeps the key. */
  onOpenInNewTab?: (row: Row, index: number) => void;
  /** Space or x on the active row. `extend` is true with Shift: the selection grows to the row, as a range. */
  onToggleSelect?: (row: Row, index: number, how: { extend: boolean }) => void;
  /** Extra classes for a row, from the row: a tint for a new row, a muted one for a row that does not apply. */
  rowClassName?: (row: Row, index: number) => string | undefined;
  /** Pixels a row is tall, when neither density fits (rows of two lines). Fixed, so the list can be virtualised. */
  rowHeight?: number;
  /** Pixels the header is tall, when a column title needs more than one line. Default 32. */
  headerHeight?: number;
  footer?: TableFooter;
  /** The server has more rows than are loaded, for a screen that draws its own foot: the whole is not known (`aria-rowcount` is -1). */
  moreRows?: boolean;
  /**
   * The row the keyboard cursor starts on, by its key: for a screen that is drawn anew when the person comes back to it (a drawer
   * that has its own address, closed). It is taken once, when the rows first include it; a table that never has the row, or whose
   * person has moved the cursor, leaves the cursor alone.
   */
  initialActiveKey?: string;
  state?: "ready" | "loading" | "error";
  empty?: ReactNode;
  error?: ReactNode;
};

const HEADER_HEIGHT = 32;
const SKELETON_ROWS = 8;
// P10: a table draws only the rows in view and one beyond them when it appears, and the rest of its overscan 200 ms later (VirtualList).
// The handoff says to virtualise above 100 rows and to draw ten beyond the window; at forty swimlanes the home page of the app cost
// 1.8 ms a row to open, on the machine the gates are measured on, which is most of a route change's budget (100 ms), so a table of
// more than thirty rows is virtualised, with five rows of overscan: a table whose window moves faster than that draws shells (see
// VirtualList, `ghostWhenFast`), and the rows are 28 to 48 px.
const VIRTUALIZE_ABOVE = 30;
const OVERSCAN = 5;
const FIRST_OVERSCAN = 1;
/** A table with more columns than this, each with a `minWidth`, draws only the columns in view. */
const WIDE_COLUMNS = 12;
/** Columns kept at the left while the others scroll. */
const PINNED_COLUMNS = 1;
const COLUMN_OVERSCAN = 2;

/** A column that is drawn, and its place among all the columns (0-based). */
type Drawn<Row> = { column: Column<Row>; index: number };

type RowViewProps<Row> = {
  row: Row;
  index: number;
  drawn: readonly Drawn<Row>[];
  /** Pinned columns come first and stay at the left; the space for the columns scrolled past follows them. */
  pinned: number;
  spacer: boolean;
  active: boolean;
  selected: boolean;
  className: string | undefined;
  onActivate: ((row: Row, index: number) => void) | undefined;
};

/** A row that entered at speed: its shape, with no cells, until the scroll slows down. */
function RowShell() {
  return <div className="h-full border-b border-border bg-background" />;
}

// `unicode-bidi: plaintext` isolates the text and gives it the direction of its own first strong letter: what a <bdi> is.
/** What a click is for itself in a row, whatever the row does with a click. */
const INTERACTIVE = 'a, button, input, select, textarea, [role="checkbox"], [role="switch"], [role="radio"], [role="menuitem"]';
const CELL = "min-w-0 truncate [unicode-bidi:plaintext]";
/** The row's `gap-x-3` and `px-3`, in pixels. */
const COLUMN_GAP = 12;
const ROW_PADDING = 12;
const PINNED_CELL = "sticky left-0 z-[1] bg-inherit";

function RowViewInner<Row>({ row, index, drawn, pinned, spacer, active, selected, className, onActivate }: RowViewProps<Row>) {
  return (
    <div
      data-active={active || undefined}
      onDoubleClick={onActivate ? () => onActivate(row, index) : undefined}

      className={cn(
        "grid h-full items-center gap-x-3 border-b border-border bg-background px-3 text-sm [grid-template-columns:var(--lk-cols)] hover:bg-accent",
        // The design's selected row: the selection colour and a bar of the primary colour at its start (`aria-selected` is on the row itself).
        // The cursor draws an outline on every row it is on; it tints only a row that is not selected (the selection colour is the louder fact).
        selected ? "bg-selection shadow-[inset_2px_0_0_var(--primary)]" : active && "bg-accent",
        active && "outline-2 -outline-offset-2 outline-ring",
        className,
      )}
    >
      {drawn.map(({ column, index: columnIndex }, at) => (
        // The space for the columns scrolled past is one grid track between the pinned columns and the others.
        <Fragment key={column.id}>
          {spacer && at === pinned ? <span role="presentation" /> : null}
          <div role="gridcell" aria-colindex={columnIndex + 1} className={cn(CELL, at < pinned && PINNED_CELL, column.align === "right" && "text-right")}>
            {column.cell(row, index)}
          </div>
        </Fragment>
      ))}
    </div>
  );
}
// A generic function cannot be wrapped by `memo` without losing its parameter; the cast puts it back.
const RowView = memo(RowViewInner) as typeof RowViewInner;

/** A grid template whose tracks may all shrink to nothing: `minmax(340px,1fr)` becomes `minmax(0,1fr)` and `150px` becomes `minmax(0,150px)`. */
function shrinkable(template: string): string {
  return template.replace(/minmax\(\s*[\d.]+px\s*,/g, "minmax(0,").replace(/(^|\s)([\d.]+px)(?=\s|$)/g, "$1minmax(0,$2)");
}

function nextSort(current: SortState | null | undefined, column: string): SortState | null {
  if (!current || current.column !== column) return { column, direction: "asc" };
  return current.direction === "asc" ? { column, direction: "desc" } : null;
}

export function DataTable<Row>({
  label,
  rows,
  columns,
  rowKey,
  density = "default",
  sort = null,
  onSortChange,
  selectedKeys,
  onActivate,
  onOpenInNewTab,
  onRowClick,
  onToggleSelect,
  rowClassName,
  rowHeight: rowHeightProp,
  headerHeight: headerHeightProp,
  footer,
  moreRows = false,
  initialActiveKey,
  state = "ready",
  empty,
  error,
  className,
  ...rest
}: DataTableProps<Row>) {
  const rowHeight = rowHeightProp ?? (density === "compact" ? 28 : 32);
  const headerHeight = headerHeightProp ?? HEADER_HEIGHT;
  const tableId = useId();
  const listRef = useRef<VirtualListHandle>(null);
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const pendingFocusAt = useRef<number | null>(null);
  // The row asked for at the start, until it is found (or the person moves the cursor, or the first rows arrive without it).
  const wantedKey = useRef<string | null>(initialActiveKey ?? null);

  // Many columns, each with a width: draw those in view. The first stays; the others are a second virtualiser, sideways, over
  // the same scroll container, which counts the pinned columns as the margin before its first column.
  const wide = columns.length > WIDE_COLUMNS && columns.every((c) => c.minWidth !== undefined);
  // The width below which the table scrolls sideways. A row is a grid with 12px between its columns and 12px of padding on each side,
  // and they are part of the width: without them a flexible column ended up narrower than its minimum, and the last of forty was half
  // out of reach at the end of the scroll.
  const minContentWidth = useMemo(() => {
    const total = columns.reduce((sum, c) => sum + (c.minWidth ?? 0), 0);
    return total > 0 ? total + (columns.length - 1) * COLUMN_GAP + 2 * ROW_PADDING : undefined;
  }, [columns]);

  const pinnedColumns = useMemo(() => (wide ? columns.slice(0, PINNED_COLUMNS) : []), [wide, columns]);
  const scrolling = useMemo(() => (wide ? columns.slice(PINNED_COLUMNS) : []), [wide, columns]);
  const pinnedWidth = useMemo(() => pinnedColumns.reduce((sum, c) => sum + (c.minWidth ?? 0), 0), [pinnedColumns]);
  // Where the first column that scrolls starts in the width of the table: the padding, the pinned columns, and a gap after each.
  const scrollStart = ROW_PADDING + pinnedWidth + pinnedColumns.length * COLUMN_GAP;
  const sideways = useVirtualizer({
    horizontal: true,
    enabled: wide,
    count: scrolling.length,
    getScrollElement: () => listRef.current?.element() ?? null,
    // A column and the gap after it: the row is a grid with a gap between its tracks, so that is where the next column starts.
    estimateSize: (i) => (scrolling[i]?.minWidth ?? 0) + COLUMN_GAP,
    overscan: COLUMN_OVERSCAN,
    scrollMargin: scrollStart,
    // Before the container exists there is nothing to measure: draw a screenful of columns, and measure after mount.
    initialRect: { width: 1440, height: 0 },
  });
  // The container exists only after the first render, so the sideways virtualiser learns of it by a second one.
  const [, rendered] = useState(false);
  useEffect(() => rendered(true), []);
  const items = wide ? sideways.getVirtualItems() : [];
  const firstColumn = items[0]?.index ?? 0;
  const lastColumn = items[items.length - 1]?.index ?? -1;
  // The columns scrolled past are one track (and the gap after it), as wide as they were less that gap: with none scrolled past there
  // is no track at all, as it would add two gaps to the first column that is drawn.
  const leading = items[0] !== undefined && items[0].start > scrollStart ? items[0].start - scrollStart - COLUMN_GAP : null;
  const spacer = wide && leading !== null;

  const drawn = useMemo<Drawn<Row>[]>(() => {
    if (!wide) return columns.map((column, index) => ({ column, index }));
    const kept = pinnedColumns.map((column, index) => ({ column, index }));
    const scrollingDrawn: Drawn<Row>[] = [];
    for (let i = firstColumn; i <= lastColumn; i++) {
      const column = scrolling[i];
      if (column) scrollingDrawn.push({ column, index: PINNED_COLUMNS + i });
    }
    return [...kept, ...scrollingDrawn];
  }, [columns, wide, pinnedColumns, scrolling, firstColumn, lastColumn]);
  const pinned = pinnedColumns.length;

  // The widths, in one variable on the container: a row sets none of its own.
  const template = useMemo(() => (wide ? [...drawn.slice(0, pinned).map((d) => `${d.column.minWidth ?? 0}px`), ...(leading === null ? [] : [`${leading}px`]), ...drawn.slice(pinned).map((d) => `${d.column.minWidth ?? 0}px`)].join(" ") : columns.map((c) => c.width).join(" ")), [wide, drawn, pinned, leading, columns]);
  const styleVars = useMemo(() => ({ "--lk-cols": template }) as CSSProperties, [template]);
  // The skeleton cannot scroll sideways, so its tracks may give way, to fit the box (the table, once it has rows, scrolls instead).
  const skeletonVars = useMemo(() => ({ "--lk-cols": shrinkable(template) }) as CSSProperties, [template]);

  const rowId = useCallback((index: number) => `${tableId}-row-${index}`, [tableId]);

  const header = (
    <div role="row" aria-rowindex={1} className="grid h-full items-center gap-x-3 border-b border-border bg-muted px-3 text-xs font-medium text-muted-foreground [grid-template-columns:var(--lk-cols)]">
      {drawn.map(({ column, index: columnIndex }, at) => {
        const ariaSort = !column.sortable ? undefined : sort?.column === column.id ? (sort.direction === "asc" ? "ascending" : "descending") : "none";
        return (
          <Fragment key={column.id}>
            {spacer && at === pinned ? <span role="presentation" /> : null}
            <div role="columnheader" aria-colindex={columnIndex + 1} aria-sort={ariaSort} className={cn("min-w-0 truncate", at < pinned && PINNED_CELL, column.align === "right" && "text-right")}>
              {column.sortable ? (
                <button type="button" onClick={() => onSortChange?.(nextSort(sort, column.id))} className="inline-flex max-w-full items-center gap-1 rounded-sm font-medium hover:text-foreground">
                  <span className="truncate">{column.header}</span>
                  {sort?.column === column.id ? sort.direction === "asc" ? <ArrowUp aria-hidden className="size-3" /> : <ArrowDown aria-hidden className="size-3" /> : <ChevronsUpDown aria-hidden className="size-3 opacity-60" />}
                </button>
              ) : (
                column.header
              )}
            </div>
          </Fragment>
        );
      })}
    </div>
  );

  const move = useCallback(
    (to: number) => {
      if (rows.length === 0) return;
      const clamped = Math.max(0, Math.min(rows.length - 1, to));
      wantedKey.current = null;
      setActiveIndex(clamped);
      listRef.current?.scrollToIndex(clamped, { align: "auto" });
    },
    [rows.length],
  );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || event.altKey) return;
    // Control or Command and Enter is the one chord the table keeps; any other chord is the browser's or the shell's.
    if ((event.ctrlKey || event.metaKey) && !(event.key === "Enter" && onOpenInNewTab)) return;
    const visible = Math.max(1, Math.floor((listRef.current?.element()?.clientHeight ?? 0) / rowHeight) - 1);
    const at = activeIndex ?? -1;
    switch (event.key) {
      case "ArrowDown":
        move(at + 1);
        break;
      case "ArrowUp":
        move(at < 0 ? 0 : at - 1);
        break;
      case "PageDown":
        move(at + visible);
        break;
      case "PageUp":
        move(Math.max(0, at - visible));
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(rows.length - 1);
        break;
      case "Enter": {
        const row = activeIndex === null ? undefined : rows[activeIndex];
        if (row === undefined || activeIndex === null) return;
        if (event.ctrlKey || event.metaKey) {
          if (!onOpenInNewTab) return;
          onOpenInNewTab(row, activeIndex);
        } else {
          if (!onActivate) return;
          onActivate(row, activeIndex);
        }
        break;
      }
      case " ":
      case "x": {
        const row = activeIndex === null ? undefined : rows[activeIndex];
        if (row === undefined || activeIndex === null || !onToggleSelect) return;
        onToggleSelect(row, activeIndex, { extend: event.shiftKey });
        break;
      }
      default:
        return;
    }
    event.preventDefault();
  };

  // A table that comes back (a drawer closed, the queue drawn anew) has the cursor on the row the person had open.
  useEffect(() => {
    const wanted = wantedKey.current;
    if (wanted === null || rows.length === 0) return;
    wantedKey.current = null;
    const at = rows.findIndex((row, i) => rowKey(row, i) === wanted);
    if (at < 0) return;
    setActiveIndex(at);
    listRef.current?.scrollToIndex(at, { align: "auto" });
  }, [rows, rowKey]);

  // "Load more" keeps the keyboard where the person was reading: on the first row that arrived.
  const rowCount = rows.length;
  useEffect(() => {
    const at = pendingFocusAt.current;
    if (at === null || rowCount <= at) return;
    pendingFocusAt.current = null;
    setActiveIndex(at);
    listRef.current?.element()?.focus({ preventScroll: true });
    listRef.current?.scrollToIndex(at, { align: "start" });
  }, [rowCount]);

  const renderRow = useCallback(
    (index: number, ghost: boolean) => {
      const row = rows[index];
      if (ghost) return <RowShell />;
      if (row === undefined) return null;
      const selected = selectedKeys?.has(rowKey(row, index)) === true;
      return <RowView row={row} index={index} drawn={drawn} pinned={pinned} spacer={spacer} active={index === activeIndex} selected={selected} className={rowClassName?.(row, index)} onActivate={onActivate} />;
    },
    [rows, drawn, pinned, spacer, activeIndex, selectedKeys, rowKey, rowClassName, onActivate],
  );
  const getRowProps = useCallback(
    (index: number) => {
      const row = rows[index];
      const selected = row !== undefined && selectedKeys?.has(rowKey(row, index)) === true;
      return {
        id: rowId(index),
        "aria-selected": selected,
        ...(onRowClick === undefined
          ? {}
          : {
              className: "cursor-pointer",
              // On the row's own element, so a click anywhere on it counts. A click on a link, a button or a field in the row is that
              // control's, not the row's.
              onClick: (event: MouseEvent<HTMLDivElement>) => {
                if (row === undefined || (event.target instanceof Element && event.target.closest(INTERACTIVE) !== null)) return;
                wantedKey.current = null;
                setActiveIndex(index);
                onRowClick(row, index);
              },
            }),
      };
    },
    [rows, selectedKeys, rowKey, rowId, onRowClick],
  );
  const getRowKey = useCallback((index: number) => {
    const row = rows[index];
    return row === undefined ? index : rowKey(row, index);
  }, [rows, rowKey]);

  // How many rows there are in all: what the server said, or "not known" (-1, as ARIA asks) while it has more pages or cut the list
  // off, so a screen reader does not take the rows loaded so far for all of them.
  const totalRows = footer === undefined ? (moreRows ? -1 : undefined) : footer.total !== undefined ? footer.total : footer.hasMore || footer.truncated === true ? -1 : undefined;

  const message = state === "loading" ? null : state === "error" ? (error ?? "The table could not be loaded.") : rows.length === 0 ? (empty ?? "Nothing to show.") : undefined;

  let body: ReactNode;
  if (state === "loading") {
    body = (
      <div role="grid" aria-label={label} aria-busy="true" aria-rowcount={SKELETON_ROWS + 1} style={skeletonVars} className="min-h-0 flex-1 overflow-hidden">
        <div style={{ height: headerHeight }}>{header}</div>
        {Array.from({ length: SKELETON_ROWS }, (_, i) => (
          <div key={i} role="row" aria-rowindex={i + 2} style={{ height: rowHeight }} className="grid items-center gap-x-3 border-b border-border px-3 [grid-template-columns:var(--lk-cols)]">
            {drawn.map(({ column, index: columnIndex }, at) => (
              <Fragment key={column.id}>
                {spacer && at === pinned ? <span role="presentation" /> : null}
                <div role="gridcell" aria-colindex={columnIndex + 1}>
                  <Skeleton className="h-3 w-2/3" />
                </div>
              </Fragment>
            ))}
          </div>
        ))}
      </div>
    );
  } else if (message !== undefined) {
    body = (
      <div role="grid" aria-label={label} aria-rowcount={2} style={styleVars} className="min-h-0 flex-1 overflow-auto">
        <div style={{ height: headerHeight }}>{header}</div>
        <div role="row" aria-rowindex={2}>
          <div role="gridcell" aria-colspan={columns.length} className="p-6 text-sm text-muted-foreground">
            {message}
          </div>
        </div>
      </div>
    );
  } else {
    body = (
      <VirtualList
        {...rest}
        ref={listRef}
        label={label}
        role="grid"
        count={rows.length}
        {...(totalRows === undefined ? {} : { totalRows })}
        rowHeight={rowHeight}
        renderRow={renderRow}
        getRowProps={getRowProps}
        getRowKey={getRowKey}
        ghostWhenFast
        virtualizeAbove={VIRTUALIZE_ABOVE}
        overscan={OVERSCAN}
        firstOverscan={FIRST_OVERSCAN}
        pinnedIndex={activeIndex}
        header={header}
        headerHeight={headerHeight}
        {...(minContentWidth === undefined ? {} : { minContentWidth })}
        aria-colcount={columns.length}
        aria-activedescendant={activeIndex === null ? undefined : rowId(activeIndex)}
        onKeyDown={onKeyDown}
        style={styleVars}
        className="flex-1"
      />
    );
  }

  return (
    <div data-slot="data-table" className={cn("flex min-h-0 flex-col", className)}>
      {body}
      {footer ? (
        <div className="flex h-9 shrink-0 items-center gap-3 border-t border-border bg-background px-3 text-xs text-muted-foreground">
          <span aria-live="polite">
            {footer.truncated ? `Showing the first ${rows.length}. The list was cut off.` : footer.total === undefined ? `Showing ${rows.length}` : `Showing ${rows.length} of ${footer.total}`}
          </span>
          {footer.hasMore ? (
            <Button
              size="sm"
              variant="secondary"
              loading={footer.loading}
              onClick={() => {
                pendingFocusAt.current = rows.length;
                footer.onLoadMore();
              }}
            >
              Load {footer.step ?? 100} more
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
