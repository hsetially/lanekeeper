import { useVirtualizer } from "@tanstack/react-virtual";
import { Fragment, memo, useCallback, useEffect, useId, useMemo, useRef, useState, type CSSProperties, type HTMLAttributes, type KeyboardEvent, type ReactNode } from "react";
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
// activates, Space toggles selection. After "Load more" the cursor lands on the first new row and the grid has focus.
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
  /** Space on the active row. */
  onToggleSelect?: (row: Row, index: number) => void;
  footer?: TableFooter;
  state?: "ready" | "loading" | "error";
  empty?: ReactNode;
  error?: ReactNode;
};

const HEADER_HEIGHT = 32;
const SKELETON_ROWS = 8;
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
  onActivate: ((row: Row, index: number) => void) | undefined;
};

/** A row that entered at speed: its shape, with no cells, until the scroll slows down. */
function RowShell() {
  return <div className="h-full border-b border-border bg-background" />;
}

// `unicode-bidi: plaintext` isolates the text and gives it the direction of its own first strong letter: what a <bdi> is.
const CELL = "min-w-0 truncate [unicode-bidi:plaintext]";
const PINNED_CELL = "sticky left-0 z-[1] bg-inherit";

function RowViewInner<Row>({ row, index, drawn, pinned, spacer, active, onActivate }: RowViewProps<Row>) {
  return (
    <div
      data-active={active || undefined}
      onDoubleClick={onActivate ? () => onActivate(row, index) : undefined}
      className={cn("grid h-full items-center gap-x-3 border-b border-border bg-background px-3 text-sm [grid-template-columns:var(--lk-cols)] hover:bg-accent", active && "bg-accent outline-2 -outline-offset-2 outline-ring")}
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
  onToggleSelect,
  footer,
  state = "ready",
  empty,
  error,
  className,
  ...rest
}: DataTableProps<Row>) {
  const rowHeight = density === "compact" ? 28 : 32;
  const tableId = useId();
  const listRef = useRef<VirtualListHandle>(null);
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const pendingFocusAt = useRef<number | null>(null);

  const minContentWidth = useMemo(() => {
    const total = columns.reduce((sum, c) => sum + (c.minWidth ?? 0), 0);
    return total > 0 ? total : undefined;
  }, [columns]);

  // Many columns, each with a width: draw those in view. The first stays; the others are a second virtualiser, sideways, over
  // the same scroll container, which counts the pinned columns as the margin before its first column.
  const wide = columns.length > WIDE_COLUMNS && columns.every((c) => c.minWidth !== undefined);
  const pinnedColumns = useMemo(() => (wide ? columns.slice(0, PINNED_COLUMNS) : []), [wide, columns]);
  const scrolling = useMemo(() => (wide ? columns.slice(PINNED_COLUMNS) : []), [wide, columns]);
  const pinnedWidth = useMemo(() => pinnedColumns.reduce((sum, c) => sum + (c.minWidth ?? 0), 0), [pinnedColumns]);
  const sideways = useVirtualizer({
    horizontal: true,
    enabled: wide,
    count: scrolling.length,
    getScrollElement: () => listRef.current?.element() ?? null,
    estimateSize: (i) => scrolling[i]?.minWidth ?? 0,
    overscan: COLUMN_OVERSCAN,
    scrollMargin: pinnedWidth,
    // Before the container exists there is nothing to measure: draw a screenful of columns, and measure after mount.
    initialRect: { width: 1440, height: 0 },
  });
  // The container exists only after the first render, so the sideways virtualiser learns of it by a second one.
  const [, rendered] = useState(false);
  useEffect(() => rendered(true), []);
  const items = wide ? sideways.getVirtualItems() : [];
  const firstColumn = items[0]?.index ?? 0;
  const lastColumn = items[items.length - 1]?.index ?? -1;
  const leading = items[0] ? items[0].start - pinnedWidth : 0;

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
  const template = useMemo(() => (wide ? [...drawn.slice(0, pinned).map((d) => `${d.column.minWidth ?? 0}px`), `${leading}px`, ...drawn.slice(pinned).map((d) => `${d.column.minWidth ?? 0}px`)].join(" ") : columns.map((c) => c.width).join(" ")), [wide, drawn, pinned, leading, columns]);
  const styleVars = useMemo(() => ({ "--lk-cols": template }) as CSSProperties, [template]);

  const rowId = useCallback((index: number) => `${tableId}-row-${index}`, [tableId]);

  const header = (
    <div role="row" aria-rowindex={1} className="grid h-full items-center gap-x-3 border-b border-border bg-muted px-3 text-xs font-medium text-muted-foreground [grid-template-columns:var(--lk-cols)]">
      {drawn.map(({ column, index: columnIndex }, at) => {
        const ariaSort = !column.sortable ? undefined : sort?.column === column.id ? (sort.direction === "asc" ? "ascending" : "descending") : "none";
        return (
          <Fragment key={column.id}>
            {wide && at === pinned ? <span role="presentation" /> : null}
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
      setActiveIndex(clamped);
      listRef.current?.scrollToIndex(clamped, { align: "auto" });
    },
    [rows.length],
  );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey) return;
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
        if (row === undefined || activeIndex === null || !onActivate) return;
        onActivate(row, activeIndex);
        break;
      }
      case " ": {
        const row = activeIndex === null ? undefined : rows[activeIndex];
        if (row === undefined || activeIndex === null || !onToggleSelect) return;
        onToggleSelect(row, activeIndex);
        break;
      }
      default:
        return;
    }
    event.preventDefault();
  };

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
      return row === undefined ? null : <RowView row={row} index={index} drawn={drawn} pinned={pinned} spacer={wide} active={index === activeIndex} onActivate={onActivate} />;
    },
    [rows, drawn, pinned, wide, activeIndex, onActivate],
  );
  const getRowProps = useCallback(
    (index: number) => {
      const row = rows[index];
      const selected = row !== undefined && selectedKeys?.has(rowKey(row, index)) === true;
      return { id: rowId(index), "aria-selected": selected };
    },
    [rows, selectedKeys, rowKey, rowId],
  );
  const getRowKey = useCallback((index: number) => {
    const row = rows[index];
    return row === undefined ? index : rowKey(row, index);
  }, [rows, rowKey]);

  const message = state === "loading" ? null : state === "error" ? (error ?? "The table could not be loaded.") : rows.length === 0 ? (empty ?? "Nothing to show.") : undefined;

  let body: ReactNode;
  if (state === "loading") {
    body = (
      <div role="grid" aria-label={label} aria-busy="true" aria-rowcount={SKELETON_ROWS + 1} style={styleVars} className="min-h-0 flex-1 overflow-hidden">
        <div style={{ height: HEADER_HEIGHT }}>{header}</div>
        {Array.from({ length: SKELETON_ROWS }, (_, i) => (
          <div key={i} role="row" aria-rowindex={i + 2} style={{ height: rowHeight }} className="grid items-center gap-x-3 border-b border-border px-3 [grid-template-columns:var(--lk-cols)]">
            {drawn.map(({ column, index: columnIndex }, at) => (
              <Fragment key={column.id}>
                {wide && at === pinned ? <span role="presentation" /> : null}
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
        <div style={{ height: HEADER_HEIGHT }}>{header}</div>
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
        rowHeight={rowHeight}
        renderRow={renderRow}
        getRowProps={getRowProps}
        getRowKey={getRowKey}
        ghostWhenFast
        pinnedIndex={activeIndex}
        header={header}
        headerHeight={HEADER_HEIGHT}
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
