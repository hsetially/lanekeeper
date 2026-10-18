import { memo, useCallback, useEffect, useMemo, useRef, type ReactNode } from "react";
import type { Mask } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { VirtualList, type VirtualListHandle } from "@/components/app/virtual/VirtualList";
import { cn } from "@/lib/utils";
import { lineMasks, splitLfLines, type LinePlace } from "./masks";

// The read-only view of a text the server masked (S14b): the same rows as CodeView (18 px, a number and the line, drawn through
// the one virtualiser), with each marker the server placed drawn as a marker. The web masks nothing itself: the text already holds
// `[REDACTED: N chars, rule]` where a value was, and `masks` only says where. Everything is data: a line goes through SafeText.
//
// Lines are numbered as the contract numbers them (a line feed ends one), which is also how the masks name their lines.

const LINE_HEIGHT = 18;
const MAX_WIDTH_CH = 2_000;

type Props = {
  label: string;
  text: string;
  masks: readonly Mask[];
  highlightLine?: number | null;
  /** A place beside each marker, for the one action a marker has (reveal, T5). Gets the mask it sits on. */
  maskAction?: (mask: Mask) => ReactNode;
  className?: string;
};

function Segments({ line, places, maskAction }: { line: string; places: readonly LinePlace[]; maskAction: Props["maskAction"] }) {
  if (places.length === 0) return <SafeText text={line} />;
  const parts: ReactNode[] = [];
  let at = 0;
  places.forEach((place, i) => {
    if (place.start > at) parts.push(<SafeText key={`t${i}`} text={line.slice(at, place.start)} />);
    parts.push(
      <span key={`m${i}`} className="inline-flex items-center gap-1.5">
        <span data-slot="mask" title="Masked by the server: the value is not in this text" className="rounded-sm border border-dashed border-input bg-muted px-1 text-muted-foreground">
          <SafeText text={line.slice(place.start, place.end)} />
        </span>
        {maskAction ? maskAction(place.mask) : null}
      </span>,
    );
    at = place.end;
  });
  if (at < line.length) parts.push(<SafeText key="tail" text={line.slice(at)} />);
  return <>{parts}</>;
}

const Row = memo(function Row({ no, line, places, highlighted, maskAction }: { no: number; line: string; places: readonly LinePlace[]; highlighted: boolean; maskAction: Props["maskAction"] }) {
  return (
    <div data-highlight={highlighted} className={cn("grid h-full font-mono text-mono [grid-template-columns:44px_minmax(0,1fr)]", highlighted && "bg-diff-mod-bg")}>
      <div role="cell" className={cn("select-none pr-2 text-right text-muted-foreground", highlighted ? "bg-diff-mod-gutter font-semibold text-diff-mod-fg" : "bg-muted")}>
        {no}
      </div>
      <div role="cell" className="overflow-hidden whitespace-pre pl-2">
        <Segments line={line} places={places} maskAction={maskAction} />
      </div>
    </div>
  );
});

const NO_PLACES: readonly LinePlace[] = [];

export function MaskedCodeView({ label, text, masks, highlightLine = null, maskAction, className }: Props) {
  const listRef = useRef<VirtualListHandle>(null);
  const lines = useMemo(() => splitLfLines(text), [text]);
  const places = useMemo(() => lineMasks(text, masks), [text, masks]);
  const widest = useMemo(() => Math.min(MAX_WIDTH_CH, lines.reduce((max, line) => Math.max(max, line.length), 0)), [lines]);
  const pinned = highlightLine !== null && highlightLine >= 1 && highlightLine <= lines.length ? highlightLine - 1 : null;

  useEffect(() => {
    if (pinned !== null) listRef.current?.scrollToIndex(pinned, { align: "center" });
  }, [pinned]);

  const renderRow = useCallback(
    (index: number) => <Row no={index + 1} line={lines[index] ?? ""} places={places.get(index + 1) ?? NO_PLACES} highlighted={index === pinned} maskAction={maskAction} />,
    [lines, places, pinned, maskAction],
  );

  if (lines.length === 0) return <div className={cn("p-6 text-sm text-muted-foreground", className)}>This file is empty.</div>;

  return (
    <VirtualList
      ref={listRef}
      label={label}
      role="table"
      rowRole="row"
      count={lines.length}
      rowHeight={LINE_HEIGHT}
      renderRow={renderRow}
      pinnedIndex={pinned}
      rowIndexOffset={0}
      minContentWidth={`calc(${widest}ch + 60px)`}
      {...(className === undefined ? {} : { className })}
    />
  );
}
