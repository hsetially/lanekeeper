import { memo, useCallback, useEffect, useMemo, useRef, type ReactNode } from "react";
import type { Mask } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { SafeText } from "@/components/app/SafeText";
import { VirtualList, type VirtualListHandle } from "@/components/app/virtual/VirtualList";
import { Lock } from "@/icons";
import { cn } from "@/lib/utils";
import { HiddenLine, HiddenValue } from "./HiddenValue";
import { lfLines, planMasks, segmentsOf, type LinePlan, type PlannedMark, type RefusalReason } from "./plan";

// The read-only view of a text the server masked (S14b, M1): the rows of the code view (18 px, a number and the line, drawn through
// the one virtualiser), with each marker drawn as a "Hidden value" token. The web masks nothing itself: the text already holds
// `[REDACTED: N chars, rule]` where a value was, and `masks` only says where (plan.ts). Everything is data: a line goes through
// SafeText, and none of it ever becomes markup.
//
// It FAILS CLOSED. A mask it cannot trust hides its whole line; a mask it cannot place refuses the text, with a notice. The view is
// the same for `FileContent`, `ServedFile` and `DocText`: the offsets are relative to the `text` of the object that carries them.
//
// `renderMask` is the one place a caller can put something in the marker's place (the Reveal control and the revealed value, T5);
// `maskAction` is the S3 slot of the same name, a place beside the token.

const LINE_HEIGHT = 18;
/**
 * A line that carries a control (a Reveal, a Hide) is a target of 24 px high. The controls are small (18 px) and secrets sit on
 * consecutive lines, so in lines of 18 px two controls would be 18 px apart where WCAG 2.5.8 asks an undersized target for 24 px of
 * clear space around it. A line with no control stays 18 px, so a long file keeps its density.
 */
const CONTROL_LINE_HEIGHT = 24;
const MAX_WIDTH_CH = 2_000;

type Props = {
  label: string;
  text: string;
  /** The server's masks for `text`. Absent means none were supplied (M1). */
  masks?: readonly Mask[] | undefined;
  /**
   * Characters that a caller puts in a marker's place and after it (a revealed value, its Hide button, its note): the rows are wider by
   * this, so the view scrolls sideways to them instead of cutting them off at the width of the masked text.
   */
  extraWidthCh?: number | undefined;
  /** The number of the first line, for a slice of a document (`DocText.lines.start`). The masks keep the text's own numbering. */
  startLine?: number;
  /** The line the screen points at, in the numbering the view shows. */
  highlightLine?: number | null;
  /** Replaces the token of a marker the masks placed. Gets the token it would have drawn. */
  renderMask?: (mark: PlannedMark, token: ReactNode) => ReactNode;
  /** A place beside each placed marker, for an action. Gets the mask it sits on. */
  maskAction?: (mask: Mask) => ReactNode;
  className?: string;
};

const REFUSAL: Record<RefusalReason, string> = {
  too_many: "The hub listed more hidden values than a text can have.",
  malformed: "The hub's list of hidden values for it is not valid.",
  missing_line: "The hub's list of hidden values names a line it does not have.",
  outside_line: "The hub's list of hidden values points outside the line it names.",
};

function Marks({ line, plan, renderMask, maskAction }: { line: string; plan: LinePlan | undefined; renderMask: Props["renderMask"]; maskAction: Props["maskAction"] }) {
  const segments = segmentsOf(line, plan);
  const [only] = segments;
  if (segments.length === 1 && only?.kind === "text") return <SafeText text={line} />;
  return (
    <>
      {segments.map((segment, i) => {
        if (segment.kind === "text") return <SafeText key={i} text={segment.text} />;
        if (segment.kind === "hidden") return <HiddenLine key={i} />;
        const token = <HiddenValue info={segment.info} setting={segment.planned?.setting ?? null} />;
        if (segment.planned === null) return <span key={i}>{token}</span>;
        const placed = segment.planned;
        const beside = maskAction ? maskAction(placed.mask) : null;
        return (
          <span key={i} className="inline-flex items-center gap-1.5">
            {renderMask ? renderMask(placed, token) : token}
            {beside}
          </span>
        );
      })}
    </>
  );
}

const Row = memo(function Row({
  no,
  line,
  plan,
  highlighted,
  renderMask,
  maskAction,
}: {
  no: number;
  line: string;
  plan: LinePlan | undefined;
  highlighted: boolean;
  renderMask: Props["renderMask"];
  maskAction: Props["maskAction"];
}) {
  return (
    <div data-highlight={highlighted} className={cn("grid h-full font-mono text-mono [grid-template-columns:44px_minmax(0,1fr)]", highlighted && "bg-diff-mod-bg")}>
      <div role="cell" className={cn("select-none pr-2 text-right text-muted-foreground", highlighted ? "bg-diff-mod-gutter font-semibold text-diff-mod-fg" : "bg-muted")}>
        {no}
      </div>
      <div role="cell" className="flex items-center overflow-hidden pl-2 whitespace-pre">
        <span className="min-w-0 overflow-hidden">
          <Marks line={line} plan={plan} renderMask={renderMask} maskAction={maskAction} />
        </span>
      </div>
    </div>
  );
});

const stripCarriageReturn = (line: string): string => (line.endsWith("\r") ? line.slice(0, -1) : line);

export function MaskedTextView({ label, text, masks, extraWidthCh = 0, startLine = 1, highlightLine = null, renderMask, maskAction, className }: Props) {
  const listRef = useRef<VirtualListHandle>(null);
  const rows = useMemo(() => lfLines(text), [text]);
  const plan = useMemo(() => planMasks(text, masks), [text, masks]);
  const widest = useMemo(() => Math.min(MAX_WIDTH_CH, rows.reduce((max, line) => Math.max(max, line.length), 0)) + Math.max(0, Math.ceil(extraWidthCh)), [rows, extraWidthCh]);
  const index = highlightLine === null ? -1 : highlightLine - startLine;
  const pinned = index >= 0 && index < rows.length ? index : null;

  useEffect(() => {
    if (pinned !== null) listRef.current?.scrollToIndex(pinned, { align: "center" });
  }, [pinned]);

  const lines = plan.status === "ok" ? plan.lines : undefined;
  // The lines that carry a control: one that `maskAction` is offered for (any placed marker), or one where `renderMask` can put a
  // control in the marker's place (a marker that names its setting: that is what a reveal needs). The function changes only with them.
  const hasAction = maskAction !== undefined;
  const hasRender = renderMask !== undefined;
  const rowHeight = useMemo((): number | ((i: number) => number) => {
    if (lines === undefined || (!hasAction && !hasRender)) return LINE_HEIGHT;
    const tall = new Set<number>();
    for (const [line, plan_] of lines) {
      if (plan_.kind === "marks" && plan_.marks.some((mark) => hasAction || mark.setting !== null)) tall.add(line - 1);
    }
    return tall.size === 0 ? LINE_HEIGHT : (i: number) => (tall.has(i) ? CONTROL_LINE_HEIGHT : LINE_HEIGHT);
  }, [lines, hasAction, hasRender]);
  const renderRow = useCallback(
    (i: number) => <Row no={startLine + i} line={stripCarriageReturn(rows[i] ?? "")} plan={lines?.get(i + 1)} highlighted={i === pinned} renderMask={renderMask} maskAction={maskAction} />,
    [rows, lines, startLine, pinned, renderMask, maskAction],
  );

  if (plan.status === "refused") {
    return (
      <div className={cn("flex items-center justify-center", className)}>
        <EmptyState icon={Lock} title="This text can't be shown safely">
          {`${REFUSAL[plan.reason]} None of the text is shown. Reload the page; if it happens again, tell an Admin.`}
        </EmptyState>
      </div>
    );
  }
  if (rows.length === 0) return <div className={cn("p-6 text-sm text-muted-foreground", className)}>This file is empty.</div>;

  return (
    <VirtualList
      ref={listRef}
      label={label}
      role="table"
      rowRole="row"
      count={rows.length}
      rowHeight={rowHeight}
      renderRow={renderRow}
      pinnedIndex={pinned}
      rowIndexOffset={0}
      minContentWidth={`calc(${widest}ch + 60px)`}
      {...(className === undefined ? {} : { className })}
    />
  );
}
