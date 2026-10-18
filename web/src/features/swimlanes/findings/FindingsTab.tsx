import { useCallback, useMemo, useRef, useState } from "react";
import type { Finding, FindingKind, Severity, SwimlaneDetail } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { VirtualList } from "@/components/app/virtual/VirtualList";
import { findingListQuery } from "@/features/findings/queries";
import { ShieldCheck } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import { findingTitle } from "../format";
import { SeverityBadge } from "./SeverityBadge";
import { byUrgency, codeOf, lineLabel } from "./severity";

// The Findings tab: what the checks (C1 to C12) found on this swimlane, the most severe first. The list is the server's, filtered
// to the swimlane; the lines link to the file at that line. Acting on a finding (marking it, restoring a lost version) is the
// Findings screen's, not this tab's.

// Four columns where there is room for them (the design's). The swimlane's right-hand panel is 360px wide at 1024 and can be dragged
// narrower, and there the Finding column would be 12px: below NARROW the lines go under the title instead, and the rows are taller.
const NARROW = 560;
const ROW = 96;
const ROW_NARROW = 148;
const WIDE_COLUMNS = "grid-cols-[44px_96px_minmax(0,1fr)_140px]";
const NARROW_COLUMNS = "grid-cols-[44px_96px_minmax(0,1fr)]";
const MAX_LINES = 6;
const MAX_LINES_NARROW = 4;

/** The width of an element, kept up to date. Zero until it is measured (and in a test that has no layout): that counts as wide. */
function useWidth(): [(element: HTMLElement | null) => void, number] {
  const [width, setWidth] = useState(0);
  const observer = useRef<ResizeObserver | null>(null);
  const ref = useCallback((element: HTMLElement | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (element === null) return;
    const next = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w !== undefined) setWidth(Math.round(w));
    });
    next.observe(element);
    observer.current = next;
  }, []);
  return [ref, width];
}

function Lines({ lane, finding, narrow }: { lane: string; finding: Finding; narrow: boolean }) {
  if (finding.path === null) return null;
  const shown = finding.locations.slice(0, narrow ? MAX_LINES_NARROW : MAX_LINES);
  const rest = finding.locations.length - shown.length;
  return (
    // Narrow, the third column is the last: the lines take its second row.
    <span className={cn("flex flex-wrap gap-1.5", narrow && "col-start-3")}>
      {shown.map((location, i) => (
        <AppLink
          key={`${location.path}:${i}`}
          route="files"
          params={{ id: lane }}
          search={{ path: finding.path ?? "", line: String(location.lines.start) }}
          title={location.path}
          className="inline-flex h-[22px] items-center rounded-sm border border-border bg-muted px-1.5 font-mono text-xs text-foreground no-underline hover:bg-accent"
        >
          {lineLabel(location.lines)}
        </AppLink>
      ))}
      {rest > 0 ? <span className="inline-flex h-[22px] items-center text-xs text-muted-foreground">+{rest} more</span> : null}
    </span>
  );
}

function FindingRow({ lane, finding, narrow }: { lane: string; finding: Finding; narrow: boolean }) {
  return (
    <div className={cn("grid h-full items-start gap-x-3 gap-y-0.5 overflow-hidden border-b border-border px-4 py-2", narrow ? NARROW_COLUMNS : WIDE_COLUMNS)}>
      <span className="pt-px font-mono text-xs font-medium">{codeOf(finding)}</span>
      <SeverityBadge severity={finding.severity} className="justify-self-start" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <strong className="font-semibold">{findingTitle(finding.kind)}</strong>
        {finding.path !== null ? (
          <AppLink route="files" params={{ id: lane }} search={{ path: finding.path }} className="min-w-0 truncate font-mono text-xs">
            <SafeText text={finding.path} />
          </AppLink>
        ) : null}
        <span className="line-clamp-2 text-muted-foreground">
          <SafeText text={finding.message} />
        </span>
      </span>
      <Lines lane={lane} finding={finding} narrow={narrow} />
    </div>
  );
}

export function FindingsTab({ lane, kinds, minSeverity }: { lane: SwimlaneDetail; kinds?: FindingKind[] | undefined; minSeverity?: Severity | undefined }) {
  const pages = useCursorPages({ ...findingListQuery({ swimlane: lane.id, ...(kinds === undefined ? {} : { kinds }), ...(minSeverity === undefined ? {} : { min_severity: minSeverity }) }), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const ordered = useMemo(() => byUrgency(pages.items), [pages.items]);
  const [measure, width] = useWidth();
  const narrow = width > 0 && width < NARROW;
  return (
    <QueryBoundaryView
      state={state}
      noun="findings"
      source="the findings list"
      className="flex min-h-0 flex-1 flex-col"
      empty={
        <EmptyState icon={ShieldCheck} title="No open findings" className="flex-1">
          Every check passes on <SafeText text={lane.display_name} className="font-mono" />.
        </EmptyState>
      }
    >
      {() => (
        <div ref={measure} data-slot="findings-tab" className="flex min-h-0 flex-1 flex-col">
          {pages.truncated ? <TruncationBanner shown={ordered.length} noun="findings" /> : null}
          <div className={cn("grid h-8 flex-none items-center gap-3 border-b border-border bg-muted px-4 text-xs font-medium text-muted-foreground", narrow ? NARROW_COLUMNS : WIDE_COLUMNS)}>
            <span>Check</span>
            <span className="text-foreground">Severity</span>
            <span>Finding</span>
            {narrow ? null : <span>Lines</span>}
          </div>
          <VirtualList
            label="Findings"
            role="list"
            className="flex-1"
            count={ordered.length}
            rowHeight={narrow ? ROW_NARROW : ROW}
            getRowKey={(i) => ordered[i]?.id ?? i}
            renderRow={(i) => {
              const finding = ordered[i];
              return finding === undefined ? null : <FindingRow lane={lane.id} finding={finding} narrow={narrow} />;
            }}
          />
        </div>
      )}
    </QueryBoundaryView>
  );
}
