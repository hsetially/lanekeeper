import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef } from "react";
import type { Comparison, GridRow } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { CopyValue } from "@/components/app/CopyValue";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { TierChip } from "@/components/app/TierBadge";
import { DiffView } from "@/components/app/diff/DiffView";
import { Banner } from "@/components/app/Banner";
import { Button } from "@/components/ui/button";
import { ArrowLeftRight, Info, Lock, X } from "@/icons";
import { useShortcut } from "@/keyboard";
import { useQueryState } from "@/lib/query/use-query-state";
import { cellKind } from "../model";
import { compareFileQuery } from "../queries";
import { hunksAround, settingSummary } from "./drawer";
import { isOffline, laneTitle, type LaneColumn } from "./lanes";

// "Diff at setting": a side drawer over the grid that shows two swimlanes at one setting. The two cells picked in a row are
// two swimlanes; the drawer asks the server to compare their effective files (`effective:` references) and shows the hunks
// that touch the setting, with the server's own words for what it found. Read only: nothing here writes.

type Props = {
  file: string;
  channel: string | null;
  row: GridRow;
  a: { lane: LaneColumn; index: number };
  b: { lane: LaneColumn; index: number };
  onClose: () => void;
  onSwap: () => void;
};

function Side({ letter, lane }: { letter: "A" | "B"; lane: LaneColumn }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 px-3 py-2 first:border-r first:border-border" title={laneTitle(lane)}>
      <span className="flex min-w-0 items-center gap-1.5">
        <span aria-hidden="true" className="inline-flex size-4 shrink-0 items-center justify-center rounded-sm bg-primary text-xs font-semibold text-primary-foreground">
          {letter}
        </span>
        {lane.tier === null ? null : <TierChip tier={lane.tier} />}
        <SafeText text={lane.id} className="truncate font-mono text-xs font-medium" />
        {lane.agent === null ? null : <AgentDot state={lane.agent.state} />}
        {isOffline(lane) ? <span className="text-xs text-muted-foreground">offline, last scan</span> : null}
      </span>
      <span className="truncate font-mono text-xs text-muted-foreground">
        {lane.tenants.length === 0 ? "no tenants" : <>tenants <SafeText text={lane.tenants.join(", ")} /></>}
        {lane.baseTag === null ? null : <> · base <SafeText text={lane.baseTag} /></>}
      </span>
    </div>
  );
}

function Body({ comparison, row, a, b }: { comparison: Comparison; row: GridRow; a: Props["a"]; b: Props["b"] }) {
  const leftLines = row.cells[a.index]?.location?.lines ?? null;
  const rightLines = row.cells[b.index]?.location?.lines ?? null;
  const summary = settingSummary(row.path, comparison, a.lane.id, b.lane.id);
  const hidden = [a.index, b.index].some((index) => {
    const cell = row.cells[index];
    return cell !== undefined && cellKind(cell) === "hidden";
  });
  const hunks = useMemo(() => hunksAround(comparison.hunks, leftLines, rightLines), [comparison.hunks, leftLines, rightLines]);
  return (
    <>
      <p className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2 text-xs text-muted-foreground">
        {leftLines === null && rightLines === null ? null : (
          <span className="inline-flex h-5 items-center rounded-sm border border-border px-1.5 font-mono text-foreground">
            {leftLines === null ? `${b.lane.id} L${rightLines?.start ?? 0}` : rightLines === null || (leftLines.start === rightLines.start && leftLines.end === rightLines.end) ? `L${leftLines.start}–${leftLines.end}` : `L${leftLines.start}–${leftLines.end} · L${rightLines.start}–${rightLines.end}`}
          </span>
        )}
        <span role="status">{summary.text}</span>
      </p>
      {comparison.eol_only ? (
        <Banner variant="info" title="Line endings only">
          The two files differ only in their line endings.
        </Banner>
      ) : null}
      {hidden ? (
        <div role="note" className="flex items-center gap-2 border-b border-info-border bg-info-bg px-4 py-2 text-info-fg">
          <Lock aria-hidden="true" className="size-3.5 shrink-0" />
          <span>A hidden value is never shown, but comparing two of them tells whether they are equal: here the server found them {summary.same ? "equal" : "different"}.</span>
        </div>
      ) : summary.same ? (
        <div role="status" className="flex items-center gap-2 border-b border-info-border bg-info-bg px-4 py-2 text-info-fg">
          <Info aria-hidden="true" className="size-3.5 shrink-0" />
          <span>
            {a.lane.id} and {b.lane.id} read the same value here.
          </span>
        </div>
      ) : null}
      <DiffView
        label="Diff at setting"
        hunks={hunks}
        mode="split"
        className="min-h-40 flex-1 border-b border-border"
        empty={summary.same ? "Nothing differs near this setting." : "The server did not send the lines around this setting."}
      />
    </>
  );
}

export function CellDrawer({ file, channel, row, a, b, onClose, onSwap }: Props) {
  const query = useQuery(compareFileQuery(`effective:${a.lane.id}`, `effective:${b.lane.id}`, file, channel));
  const state = useQueryState(query);
  const closeRef = useRef<HTMLButtonElement>(null);
  // Focus goes into the drawer when it opens and back to where it was when it closes.
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    return () => {
      if (before?.isConnected === true) before.focus();
    };
  }, []);
  useShortcut("global.close-dialog-menu-or-palette", () => {
    onClose();
  });
  return (
    <div role="dialog" aria-label="Diff at setting" className="absolute inset-y-0 right-0 z-30 flex w-[640px] max-w-full flex-col border-l border-border bg-popover text-popover-foreground shadow-popover">
      <div className="flex flex-none items-start gap-2 border-b border-border py-3 pr-3 pl-4">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <h2 className="text-base leading-[22px] font-semibold">Diff at setting</h2>
          <CopyValue value={row.path} what="setting path" valueClassName="font-mono text-xs [overflow-wrap:anywhere]" />
        </div>
        <Button ref={closeRef} variant="ghost" size="icon-sm" aria-label="Close diff" onClick={onClose}>
          <X />
        </Button>
      </div>
      <div className="grid flex-none grid-cols-2 border-b border-border">
        <Side letter="A" lane={a.lane} />
        <Side letter="B" lane={b.lane} />
      </div>
      <QueryBoundaryView state={state} noun="comparison" source={`${a.lane.id} and ${b.lane.id}`} className="flex min-h-0 flex-1 flex-col">
        {(comparison) => <Body comparison={comparison} row={row} a={a} b={b} />}
      </QueryBoundaryView>
      <div className="flex flex-none items-center gap-2 px-4 py-2">
        <Button variant="secondary" size="sm" onClick={onSwap}>
          <ArrowLeftRight data-icon="inline-start" />
          Swap A and B
        </Button>
        <span className="ml-auto text-xs text-muted-foreground">Read only. Edit from a file view.</span>
      </div>
    </div>
  );
}
