import { memo, useCallback, useEffect, useImperativeHandle, useMemo, useRef, type HTMLAttributes, type ReactNode, type Ref } from "react";
import { SafeText } from "@/components/app/SafeText";
import { cn } from "@/lib/utils";
import { VirtualList, type VirtualListHandle } from "./VirtualList";

// The read-only file viewer (P10, plan W16): lines of 18 px with a number, drawn through the one virtualiser, so a 6,000-line
// file costs the same as a short one. It is not Monaco: the editor is a lazy chunk used only by Edit mode. This is also what
// an editor chunk that failed to load falls back to.
//
// The text is data: each line goes through SafeText (hidden and bidi characters become visible markers) and never becomes
// markup. Lines are split once, on `\r\n`, `\n` or `\r`, and the line ending is not part of the line (a CRLF file would
// otherwise show a marker at the end of every line).

export type CodeViewHandle = {
  /** Scroll so that this line (in the numbering the view shows) is in the middle. */
  scrollToLine: (line: number) => void;
};

/** The lines of a text, without their endings. A text that ends with a line ending has no empty last line. */
export function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split(/\r\n|\n|\r/);
  if (lines.length > 0 && lines[lines.length - 1] === "" && /[\r\n]$/.test(text)) lines.pop();
  return lines;
}

type CodeViewProps = Omit<HTMLAttributes<HTMLDivElement>, "children" | "role" | "aria-label"> & {
  label: string;
  text: string;
  /** The number of the first line, for a view of a slice of a file. Default 1. */
  startLine?: number;
  /** The line the screen points at (a `line` search parameter): marked, and scrolled to when it changes. */
  highlightLine?: number | null;
  /** What to show for an empty file. */
  empty?: ReactNode;
  ref?: Ref<CodeViewHandle>;
};

const LINE_HEIGHT = 18;
/** A line wider than this is clipped rather than making the view scroll for a megabyte. */
const MAX_WIDTH_CH = 2_000;

const CodeRow = memo(function CodeRow({ no, text, highlighted }: { no: number; text: string; highlighted: boolean }) {
  return (
    <div data-highlight={highlighted} className={cn("grid h-full font-mono text-mono [grid-template-columns:44px_minmax(0,1fr)]", highlighted && "bg-diff-mod-bg")}>
      <div role="cell" className={cn("select-none pr-2 text-right text-muted-foreground", highlighted ? "bg-diff-mod-gutter font-semibold text-diff-mod-fg" : "bg-muted")}>
        {no}
      </div>
      <div role="cell" className="overflow-hidden whitespace-pre pl-2">
        <SafeText text={text} />
      </div>
    </div>
  );
});

export function CodeView({ label, text, startLine = 1, highlightLine = null, empty, ref, className, ...rest }: CodeViewProps) {
  const listRef = useRef<VirtualListHandle>(null);
  const lines = useMemo(() => splitLines(text), [text]);
  const widest = useMemo(() => Math.min(MAX_WIDTH_CH, lines.reduce((max, line) => Math.max(max, line.length), 0)), [lines]);

  const indexOfLine = useCallback((line: number) => line - startLine, [startLine]);
  const highlightIndex = highlightLine === null ? null : indexOfLine(highlightLine);
  const pinned = highlightIndex !== null && highlightIndex >= 0 && highlightIndex < lines.length ? highlightIndex : null;

  useImperativeHandle(
    ref,
    () => ({
      scrollToLine: (line) => {
        const index = indexOfLine(line);
        if (index >= 0 && index < lines.length) listRef.current?.scrollToIndex(index, { align: "center" });
      },
    }),
    [indexOfLine, lines.length],
  );

  useEffect(() => {
    if (pinned !== null) listRef.current?.scrollToIndex(pinned, { align: "center" });
  }, [pinned]);

  const renderRow = useCallback(
    (index: number) => <CodeRow no={startLine + index} text={lines[index] ?? ""} highlighted={index === pinned} />,
    [lines, startLine, pinned],
  );

  if (lines.length === 0) return <div className={cn("p-6 text-sm text-muted-foreground", className)}>{empty ?? "This file is empty."}</div>;

  return (
    <VirtualList
      {...rest}
      ref={listRef}
      label={label}
      role="table"
      rowRole="row"
      count={lines.length}
      rowHeight={LINE_HEIGHT}
      renderRow={renderRow}
      pinnedIndex={pinned}
      rowIndexOffset={startLine - 1}
      minContentWidth={`calc(${widest}ch + 60px)`}
      className={className}
    />
  );
}
