import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { SwimlaneSummary } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { Folder, Layers } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import type { PageProps } from "@/routes/props";
import { cn } from "@/lib/utils";
import { GridScreen } from "./grid/GridScreen";
import { LanesScreen } from "./lanes/LanesScreen";
import { compareHref, parseView, type CompareMode, type CompareView, type SearchRecord } from "./model";
import { useReplaceTo } from "@/routes/AppLink";

// Compare: one logical file across swimlanes (the grid), or one swimlane against another, a Git branch or a Git tag (the whole
// swimlane). Both are one page and one switch. What a link should keep is in the address (the keys the route reads); the
// rest of the choices live in the page for the visit.

const SUBTITLE: Record<CompareMode, string> = {
  grid: "One logical file, one column per swimlane. Each cell shows the value from the file that swimlane actually reads.",
  lanes: "Every directory and logical file in one swimlane against another swimlane, a Git branch or a Git tag.",
};

export default function ComparePage({ search }: PageProps<"compare">) {
  const replace = useReplaceTo();
  return <CompareScreen search={search} onAddress={replace} />;
}

/** The page without the router: the address it was opened with, and where it writes the address when the person changes it. */
export function CompareScreen({ search, onAddress }: { search: SearchRecord; onAddress: (href: string) => void }) {
  const fromAddress = useMemo(() => parseView(search), [search]);
  const [view, setViewState] = useState<CompareView>(fromAddress);
  const written = useRef(compareHref(fromAddress));

  // The address changed by itself (Back, a link): take it. Our own writes set `written` first, so they do not come back.
  useEffect(() => {
    const href = compareHref(fromAddress);
    if (href !== written.current) {
      written.current = href;
      setViewState(fromAddress);
    }
  }, [fromAddress]);

  const setView = useCallback(
    (next: CompareView) => {
      setViewState(next);
      const href = compareHref(next);
      if (href !== written.current) {
        written.current = href;
        onAddress(href);
      }
    },
    [onAddress],
  );

  // The swimlane list is the one the sidebar and the switcher use, so it is one cache entry shared with them and their live events.
  const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const swimlanes = useQueryState(cursorPagesSource(pages));
  const all: SwimlaneSummary[] = swimlanes.data ?? [];

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Compare</h1>
          <span className="text-muted-foreground">{SUBTITLE[view.mode]}</span>
        </div>
        <div role="group" aria-label="Compare mode" className="inline-flex overflow-hidden rounded-sm border border-input bg-background">
          <ModeButton pressed={view.mode === "grid"} onClick={() => setView({ ...view, mode: "grid" })} icon={<Layers data-icon="inline-start" />}>
            One file across swimlanes
          </ModeButton>
          <ModeButton pressed={view.mode === "lanes"} onClick={() => setView({ ...view, mode: "lanes", ...(view.left === null && view.right === null ? pairFrom(all, view.lanes) : {}) })} icon={<Folder data-icon="inline-start" />} divided>
            Whole swimlane
          </ModeButton>
        </div>
      </div>
      {view.mode === "grid" ? <GridScreen view={view} onView={setView} swimlanes={swimlanes} /> : <LanesScreen view={view} onView={setView} swimlanes={swimlanes} />}
    </div>
  );
}

/** The first two swimlanes of the grid, as the sides of a whole-swimlane comparison, so that switching shows something to compare. */
function pairFrom(all: readonly SwimlaneSummary[], lanes: readonly string[]): { left?: string; right?: string } {
  const [a, b] = lanes.length >= 2 ? lanes : all.map((lane) => lane.id);
  return a !== undefined && b !== undefined ? { left: `effective:${a}`, right: `effective:${b}` } : {};
}

function ModeButton({ pressed, onClick, icon, divided = false, children }: { pressed: boolean; onClick: () => void; icon: ReactNode; divided?: boolean; children: ReactNode }) {
  return (
    <Button variant="ghost" aria-pressed={pressed} onClick={onClick} className={cn("rounded-none border-0", divided && "border-l border-input", pressed ? "bg-secondary font-medium" : "font-normal")}>
      {icon}
      {children}
    </Button>
  );
}
