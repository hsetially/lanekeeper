import { memo, useCallback, useEffect, useId, useImperativeHandle, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent, type ReactNode, type Ref } from "react";
import type { DiffHunk } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { byteRangesToUtf16 } from "@/lib/text/byte-ranges";
import { cn } from "@/lib/utils";
import { VirtualList, type VirtualListHandle } from "../virtual/VirtualList";
import { buildDiffRows, ROW_HEIGHT, type DiffCell, type DiffRow } from "./rows";

// The diff view (P10): split and unified over server-computed hunks, drawn through the one virtualiser. Both views share
// one list of rows, so the two panes of the split view can never fall out of line. A 6,000-line file is never diffed in the
// browser: the hunks come from the engine, with the changed words inside each line (UTF-8 byte offsets, converted here).
//
// Keys: `n` and `p` move between runs of changed rows (the Shell's keys, handoff inconsistency H-1), `aria-activedescendant`
// names the row, and the active row stays rendered. Text from a file is data: every line goes through SafeText, which shows
// hidden and bidi characters as visible markers, and none of it ever becomes markup.

export type DiffViewHandle = {
  /** `1` for the next change, `-1` for the previous one. */
  goToChange: (delta: 1 | -1) => void;
  /** Put the keyboard on the diff, so `n` and `p` work. */
  focus: () => void;
};

type DiffViewProps = Omit<HTMLAttributes<HTMLDivElement>, "children" | "role" | "aria-label"> & {
  label: string;
  hunks: readonly DiffHunk[];
  mode: "split" | "unified";
  /** The length of the old file, so that a fold after the last hunk can be shown. */
  oldTotal?: number;
  /** The text of unchanged lines (`from` is 1-based), so a fold can be opened. Without it a fold is only a count. Keep it stable. */
  getLines?: (side: "old" | "new", from: number, count: number) => readonly string[] | null;
  /** What to show when there are no hunks. Default "No differences". */
  empty?: ReactNode;
  /** The index of the active change and how many there are, when `n` or `p` moves it. */
  onActiveChange?: (index: number, total: number) => void;
  ref?: Ref<DiffViewHandle>;
};

const NUMBER_W = "44px";
const MARKER_W = "16px";
const UNIFIED_COLUMNS = `${NUMBER_W} ${NUMBER_W} ${MARKER_W} minmax(0,1fr)`;
const SPLIT_COLUMNS = `${NUMBER_W} ${MARKER_W} minmax(0,1fr) ${NUMBER_W} ${MARKER_W} minmax(0,1fr)`;

const GUTTER: Record<DiffCell["kind"], string> = {
  context: "bg-muted text-muted-foreground",
  added: "bg-diff-add-gutter text-muted-foreground",
  removed: "bg-diff-del-gutter text-muted-foreground",
};
const BODY: Record<DiffCell["kind"], string> = { context: "", added: "bg-diff-add-bg", removed: "bg-diff-del-bg" };
const MARKER: Record<DiffCell["kind"], string> = { context: "", added: "text-diff-add-fg", removed: "text-diff-del-fg" };
const WORD: Record<DiffCell["kind"], string> = { context: "", added: "bg-diff-add-word", removed: "bg-diff-del-word" };

/** One line of text with its changed words marked. A span that cannot be right is dropped and the line shows plain. */
function CodeText({ cell }: { cell: DiffCell }) {
  const spans = cell.ranges.length > 0 ? byteRangesToUtf16(cell.text, cell.ranges) : [];
  if (spans.length === 0) return <SafeText text={cell.text} />;
  const parts: ReactNode[] = [];
  let at = 0;
  spans.forEach((span, i) => {
    if (span.start > at) parts.push(<SafeText key={`t${i}`} text={cell.text.slice(at, span.start)} />);
    parts.push(
      <mark key={`m${i}`} className={cn("rounded-[2px] text-inherit", WORD[cell.kind])}>
        <SafeText text={cell.text.slice(span.start, span.end)} />
      </mark>,
    );
    at = span.end;
  });
  if (at < cell.text.length) parts.push(<SafeText key="tail" text={cell.text.slice(at)} />);
  return <>{parts}</>;
}

function Gutter({ cell }: { cell: DiffCell | null }) {
  return (
    <div role="cell" className={cn("select-none pr-2 text-right", cell ? GUTTER[cell.kind] : "bg-diff-empty")}>
      {cell ? cell.no : ""}
    </div>
  );
}

function Marker({ cell }: { cell: DiffCell | null }) {
  const name = cell?.kind === "added" ? "added" : cell?.kind === "removed" ? "removed" : undefined;
  return (
    <div role="cell" {...(name ? { "aria-label": name } : {})} className={cn("select-none text-center", cell ? [BODY[cell.kind], MARKER[cell.kind]] : "bg-diff-empty")}>
      {cell?.kind === "added" ? "+" : cell?.kind === "removed" ? "-" : ""}
    </div>
  );
}

function Code({ cell }: { cell: DiffCell | null }) {
  return (
    <div role="cell" className={cn("overflow-hidden whitespace-pre", cell ? BODY[cell.kind] : "bg-diff-empty")}>
      {cell ? <CodeText cell={cell} /> : null}
    </div>
  );
}

type RowViewProps = { row: DiffRow; mode: "split" | "unified"; active: boolean; onOpenFold: ((id: string) => void) | undefined };

function DiffRowViewInner({ row, mode, active, onOpenFold }: RowViewProps) {
  if (row.kind === "hunk") {
    return (
      <div role="cell" aria-colspan={mode === "split" ? 6 : 4} className="flex h-full items-center bg-diff-hunk-bg px-3 font-mono text-mono text-diff-hunk-fg">
        {row.header}
      </div>
    );
  }
  if (row.kind === "fold") {
    const label = `${row.hidden} unchanged ${row.hidden === 1 ? "line" : "lines"}`;
    return (
      <div role="cell" aria-colspan={mode === "split" ? 6 : 4} className="flex h-full items-center bg-muted px-3 font-sans text-xs text-muted-foreground">
        {onOpenFold ? (
          <button type="button" className="rounded-sm underline-offset-2 hover:underline focus-visible:outline-offset-0" onClick={() => onOpenFold(row.id)}>
            {label}
          </button>
        ) : (
          label
        )}
      </div>
    );
  }
  const ring = active ? "outline-2 -outline-offset-2 outline-ring" : "";
  if (mode === "unified") {
    const cell = row.right ?? row.left;
    return (
      <div className={cn("grid h-full font-mono text-mono", ring)} style={{ gridTemplateColumns: UNIFIED_COLUMNS }}>
        <Gutter cell={row.left} />
        <Gutter cell={row.right} />
        <Marker cell={cell} />
        <Code cell={cell} />
      </div>
    );
  }
  return (
    <div className={cn("grid h-full font-mono text-mono", ring)} style={{ gridTemplateColumns: SPLIT_COLUMNS }}>
      <Gutter cell={row.left} />
      <Marker cell={row.left} />
      <Code cell={row.left} />
      <Gutter cell={row.right} />
      <Marker cell={row.right} />
      <Code cell={row.right} />
    </div>
  );
}
const DiffRowView = memo(DiffRowViewInner);

export function DiffView({ label, hunks, mode, oldTotal, getLines, empty, onActiveChange, ref, className, ...rest }: DiffViewProps) {
  const diffId = useId();
  const listRef = useRef<VirtualListHandle>(null);
  // Which folds are open belongs to one set of hunks: a different file starts with them closed.
  const [open, setOpen] = useState<{ hunks: readonly DiffHunk[]; ids: ReadonlySet<string> }>({ hunks, ids: new Set() });
  const expanded = open.hunks === hunks ? open.ids : EMPTY;
  const openFold = useCallback((id: string) => setOpen((current) => ({ hunks, ids: new Set([...(current.hunks === hunks ? current.ids : []), id]) })), [hunks]);

  const layout = useMemo(() => buildDiffRows(hunks, { mode, expanded, ...(oldTotal === undefined ? {} : { oldTotal }), ...(getLines ? { getLines } : {}) }), [hunks, mode, expanded, oldTotal, getLines]);
  const { rows, changeStarts } = layout;

  // The widest line, in characters, sets how far the view scrolls sideways (`ch` is exact in a monospace font).
  const widest = useMemo(() => {
    let max = 0;
    for (const row of rows) {
      if (row.kind !== "line") continue;
      for (const cell of [row.left, row.right]) if (cell && cell.text.length > max) max = cell.text.length;
    }
    return max;
  }, [rows]);
  const minContentWidth = mode === "split" ? `calc(${2 * widest}ch + 120px)` : `calc(${widest}ch + 104px)`;

  const [activeChange, setActiveChange] = useState<number | null>(null);
  const total = changeStarts.length;
  const activeRow = activeChange === null ? null : (changeStarts[Math.min(activeChange, total - 1)] ?? null);
  // The latest change, readable by two moves in one tick (a state value would be stale for the second).
  const latest = useRef<number | null>(null);
  latest.current = activeChange;
  const scrollPending = useRef(false);
  const goTo = useCallback(
    (delta: 1 | -1) => {
      if (total === 0) return;
      const current = latest.current;
      const next = current === null ? 0 : Math.max(0, Math.min(total - 1, current + delta));
      latest.current = next;
      scrollPending.current = true;
      setActiveChange(next);
    },
    [total],
  );
  useImperativeHandle(ref, () => ({ goToChange: goTo, focus: () => listRef.current?.element()?.focus({ preventScroll: true }) }), [goTo]);
  // Scroll only after a move the person asked for, not when a fold opens and the active row's index changes.
  useEffect(() => {
    if (!scrollPending.current || activeRow === null) return;
    scrollPending.current = false;
    listRef.current?.scrollToIndex(activeRow, { align: "center" });
  }, [activeRow, activeChange]);

  // Tell the screen where the person is ("3 of 12"). Not during the render that moved: the effect runs after it.
  const lastReported = useRef<number | null>(null);
  useEffect(() => {
    if (activeChange === null || lastReported.current === activeChange) return;
    lastReported.current = activeChange;
    onActiveChange?.(activeChange, total);
  }, [activeChange, total, onActiveChange]);

  const rowId = useCallback((index: number) => `${diffId}-r-${index}`, [diffId]);
  const rowHeight = useCallback((index: number) => ROW_HEIGHT[rows[index]?.kind ?? "line"], [rows]);
  const renderRow = useCallback(
    (index: number) => {
      const row = rows[index];
      return row ? <DiffRowView row={row} mode={mode} active={index === activeRow} onOpenFold={getLines ? openFold : undefined} /> : null;
    },
    [rows, mode, activeRow, getLines, openFold],
  );
  const getRowProps = useCallback((index: number) => ({ id: rowId(index) }), [rowId]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === "n") goTo(1);
    else if (event.key === "p") goTo(-1);
    else return;
    event.preventDefault();
  };

  if (hunks.length === 0) return <div className={cn("p-6 text-sm text-muted-foreground", className)}>{empty ?? "No differences"}</div>;

  return (
    <VirtualList
      {...rest}
      ref={listRef}
      label={label}
      role="grid"
      aria-readonly="true"
      rowRole="row"
      count={rows.length}
      rowHeight={rowHeight}
      renderRow={renderRow}
      getRowProps={getRowProps}
      pinnedIndex={activeRow}
      minContentWidth={minContentWidth}
      rowIndexOffset={0}
      data-active-change={activeChange === null ? "none" : String(activeChange)}
      aria-activedescendant={activeRow === null ? undefined : rowId(activeRow)}
      onKeyDown={onKeyDown}
      className={className}
    />
  );
}

const EMPTY: ReadonlySet<string> = new Set();
