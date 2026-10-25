import { useCallback } from "react";
import type { Version } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { HashChip } from "@/components/app/chips";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { VirtualList } from "@/components/app/virtual/VirtualList";
import type { GateResult } from "@/components/app/write-gate";
import { AttributionChip, SeverityBadge, type NameLookup } from "@/components/attribution";
import { Button } from "@/components/ui/button";
import { useActorNames } from "@/features/attribution";
import { GitBranch, Pencil, Search, X } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { fileHistoryQuery } from "../queries";
import { sourceLabel } from "./attribution";

// The versions of a file, newest first (History panel, 1j): when, which content, how it was seen, who changed it and how sure
// the server is (the shared attribution chip, which opens the evidence behind it), and a Revert on every version but the current one. Revert only ASKS (the screen opens its dialog); it writes
// nothing itself. A panel on the right, 400 px, like the Docs panel.

const ROW_HEIGHT = 104;

const SOURCE_ICON = { scan: Search, tool_write: Pencil, sync: GitBranch } as const;

type RowProps = { version: Version; current: boolean; nameOf: NameLookup; revertGate: GateResult; onRevert: (version: Version) => void; copyHash: boolean };

function VersionRow({ version, current, nameOf, revertGate, onRevert, copyHash }: RowProps) {
  const Icon = SOURCE_ICON[version.source];
  return (
    <div className={cn("flex h-full flex-col justify-center gap-1.5 border-b border-border px-4", current && "bg-muted")}>
      <div className="flex items-center gap-2">
        <span className="font-medium tabular-nums">{formatTime(version.observed_at)}</span>
        <HashChip hash={version.hash} copy={copyHash} />
        {current ? <span className="text-xs text-muted-foreground">Current</span> : null}
        <SeverityBadge severity={version.severity} className="ml-auto" />
      </div>
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Icon aria-hidden="true" className="size-3 shrink-0" />
        {sourceLabel(version.source)}
        {current ? null : (
          <GatedButton variant="secondary" size="sm" gate={revertGate} onClick={() => onRevert(version)} className="ml-auto" aria-label={`Revert to ${Array.from(version.hash).slice(0, 7).join("")}`}>
            Revert
          </GatedButton>
        )}
      </div>
      <AttributionChip attribution={version.attribution} nameOf={nameOf} />
    </div>
  );
}

export type HistoryPanelProps = {
  id: string;
  path: string;
  /** The hash on NFS now: the version marked Current. */
  currentHash: string | null;
  revertGate: GateResult;
  onRevert: (version: Version) => void;
  onClose: () => void;
  /** False for a file whose content is withheld: each version's hash is shown and not offered to be copied. */
  copyHash?: boolean;
};

export function HistoryPanel({ id, path, currentHash, revertGate, onRevert, onClose, copyHash = true }: HistoryPanelProps) {
  const { nameOf } = useActorNames();
  const pages = useCursorPages(fileHistoryQuery(id, path));
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const { items } = pages;
  const renderRow = useCallback(
    (index: number) => {
      const version = items[index];
      if (!version) return null;
      // The newest version is the current one unless the head says otherwise (the head may be a moment ahead of the list).
      return <VersionRow version={version} current={currentHash === null ? index === 0 : version.hash === currentHash} nameOf={nameOf} revertGate={revertGate} onRevert={onRevert} copyHash={copyHash} />;
    },
    [items, currentHash, nameOf, revertGate, onRevert, copyHash],
  );
  return (
    <aside aria-label="History" className="flex w-[400px] max-w-full flex-none flex-col border-l border-border bg-card">
      <div className="flex h-10 flex-none items-center justify-between border-b border-border pr-3 pl-4">
        <strong className="text-base font-semibold">History</strong>
        <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
          <X aria-hidden="true" />
        </Button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col">
        <QueryBoundaryView state={state} noun="history" empty={<EmptyState title="No versions recorded yet">Versions appear here once the file has been seen on NFS.</EmptyState>} className="flex min-h-0 flex-1 flex-col">
          {() => (
            <>
              <VirtualList label="Versions of this file" role="list" count={items.length} rowHeight={ROW_HEIGHT} renderRow={renderRow} className="min-h-0 flex-1" />
              {pages.hasMore ? (
                <div className="flex-none border-t border-border p-2">
                  <Button variant="secondary" className="w-full" onClick={() => void pages.loadMore()}>
                    Show older versions
                  </Button>
                </div>
              ) : null}
              {pages.truncated ? <p className="flex-none border-t border-border p-2 text-xs text-muted-foreground">Showing the newest {items.length} versions.</p> : null}
            </>
          )}
        </QueryBoundaryView>
      </div>
    </aside>
  );
}
