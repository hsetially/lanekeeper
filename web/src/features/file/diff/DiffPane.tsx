import { useMemo, useRef, useState } from "react";
import type { Comparison, EolStyle, FileContent, FileEntry, SwimlaneDetail } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { DiffView, type DiffViewHandle } from "@/components/app/diff/DiffView";
import type { GateResult } from "@/components/app/write-gate";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { ArrowDown, ArrowLeftRight, ArrowUp, Columns2, GitBranch, Rows2, Server } from "@/icons";
import { Kbd } from "@/components/ui/kbd";
import { useShortcut } from "@/keyboard";
import { useQueryState } from "@/lib/query/use-query-state";
import { lineCount } from "../model";
import { ChangeList, SettingChips } from "./ChangeSummary";
import { BinaryCompare } from "./BinaryCompare";
import { foldContext } from "./folds";
import { LineEndingNotice } from "./LineEndingNotice";
import { RefPicker } from "./RefPicker";
import type { Side } from "./refs";
import { countChangeRuns } from "./runs";
import { summarise } from "./summary";
import { useComparison } from "./useComparison";

// The Diff tab (File, 1a to 1c and 1m). The engine compares this swimlane's NFS with another version and the browser lays the
// answer out: it never reads two files to diff them (rule 7, P10). A change of line endings only is hidden behind a notice, a
// binary file is compared by picture, the settings that changed are listed, and the unchanged lines between hunks can be
// unfolded from the NFS text when the hunks agree with it.

export type DiffPaneProps = {
  lane: string;
  path: string;
  name: string;
  entry: FileEntry | null;
  detail: SwimlaneDetail | undefined;
  /** The NFS content: its text unfolds the unchanged lines, its line ending names the notice. */
  head: FileContent | undefined;
  side: Side;
  swapped: boolean;
  layout: "split" | "unified";
  onSide: (side: Side) => void;
  onSwap: () => void;
  onLayout: (layout: "split" | "unified") => void;
  convertGate: GateResult;
  onConvert: (to: "lf" | "crlf") => void;
};

const LOADING = { state: "loading", data: undefined, error: null, retryable: false, refreshing: false, stale: null, refetch: () => undefined } as const;

function Caption({ kind, label }: { kind: "git" | "nfs"; label: string }) {
  const Icon = kind === "git" ? GitBranch : Server;
  return (
    <div className="flex items-center gap-1.5 px-3 text-muted-foreground">
      <Icon aria-hidden="true" className="size-3" />
      <span className="font-mono text-foreground">{label}</span>
    </div>
  );
}

export function DiffPane({ lane, path, name, entry, detail, head, side, swapped, layout, onSide, onSwap, onLayout, convertGate, onConvert }: DiffPaneProps) {
  const { context, sides, query, labels } = useComparison({ lane, path, entry, detail, side, swapped, enabled: true });
  const state = useQueryState(query, { isEmpty: () => false });
  const diff = useRef<DiffViewHandle>(null);
  const [active, setActive] = useState<{ index: number; total: number } | null>(null);
  const [showAnyway, setShowAnyway] = useState(false);

  useShortcut("diff.toggle-split-and-unified", () => onLayout(layout === "split" ? "unified" : "split"));
  // The toolbar shows n and p on its buttons, so they work from anywhere on the tab (the first binding of the row is n, the second
  // p). The diff also handles them itself when it has focus; the registry takes the key first, so a change is never moved twice.
  useShortcut("diff.next-previous-change", (hit) => {
    diff.current?.goToChange(hit.binding === 0 ? 1 : -1);
  });

  const left = swapped ? "NFS (current)" : (labels?.chosen ?? "");
  const right = swapped ? (labels?.chosen ?? "") : "NFS (current)";
  const gitLabel = context === null ? "Git" : context.tenant !== null ? `Git (${context.tenant} head)` : `Git (${context.baseLabel})`;
  const data: Comparison | undefined = query.data;
  const runs = useMemo(() => (data === undefined ? 0 : countChangeRuns(data.hunks)), [data]);
  const folds = useMemo(() => (data === undefined || head?.text === undefined || head.text === null ? null : foldContext(data.hunks, swapped ? "old" : "new", head.text)), [data, head?.text, swapped]);
  const summary = useMemo(() => summarise(data?.setting_changes ?? []), [data]);

  const toolbar = (
    <div className="relative z-[5] flex min-h-10 flex-none flex-wrap items-center gap-x-2 gap-y-1 border-b border-border bg-muted px-4 py-1">
      {context === null ? (
        <span className="text-muted-foreground">Comparing…</span>
      ) : (
        <>
          <RefPicker label="Left" text={left} lane={lane} tenant={context.tenant} gitLabel={gitLabel} onSelect={onSide} fixed={swapped} />
          <Button variant="ghost" size="icon" aria-label="Swap sides" onClick={onSwap}>
            <ArrowLeftRight aria-hidden="true" />
          </Button>
          <RefPicker label="Right" text={right} lane={lane} tenant={context.tenant} gitLabel={gitLabel} onSelect={onSide} fixed={!swapped} />
        </>
      )}
      <ToggleGroup type="single" value={layout} onValueChange={(value) => (value === "split" || value === "unified") && onLayout(value)} aria-label="Diff layout" className="ml-2">
        <ToggleGroupItem value="split" aria-label="Side by side">
          <Columns2 aria-hidden="true" />
          Side by side
        </ToggleGroupItem>
        <ToggleGroupItem value="unified" aria-label="Unified">
          <Rows2 aria-hidden="true" />
          Unified
        </ToggleGroupItem>
      </ToggleGroup>
      {runs > 0 ? (
        <>
          <span data-slot="diff-position" aria-live="polite" className="ml-auto text-xs text-muted-foreground tabular-nums">
            {active === null ? `${runs} ${runs === 1 ? "change" : "changes"}` : `Change ${active.index + 1} of ${active.total}`}
          </span>
          <Button variant="ghost" size="sm" onClick={() => diff.current?.goToChange(-1)}>
            <ArrowUp aria-hidden="true" />
            Previous
            <Kbd>p</Kbd>
          </Button>
          <Button variant="ghost" size="sm" onClick={() => diff.current?.goToChange(1)}>
            <ArrowDown aria-hidden="true" />
            Next
            <Kbd>n</Kbd>
          </Button>
        </>
      ) : null}
    </div>
  );

  if (context !== null && sides === null) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {toolbar}
        <EmptyState title="No version to compare with">This swimlane has no base version for a base file, so there is no Git side to compare. Pick another swimlane or a tag.</EmptyState>
      </div>
    );
  }

  const nfsEol: EolStyle = head?.format.eol ?? "lf";
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {toolbar}
      <QueryBoundaryView state={context === null ? LOADING : state} noun="comparison" className="flex min-h-0 flex-1 flex-col">
        {(cmp) => {
          if (cmp.binary !== null) return <BinaryCompare name={name} leftLabel={left} rightLabel={right} binary={cmp.binary} />;
          if (cmp.identical) return <EmptyState title="No differences">{`${left} and ${right} are the same.`}</EmptyState>;
          if (cmp.eol_only && !showAnyway) {
            const other = swapped ? left : right === "NFS (current)" ? left : right;
            return (
              <LineEndingNotice
                nfsEol={nfsEol}
                lines={head?.text == null ? 0 : lineCount(head.text)}
                against={other.startsWith("Git") ? "Git" : other}
                canShowAnyway={cmp.hunks.length > 0}
                convertGate={convertGate}
                onShowAnyway={() => setShowAnyway(true)}
                onConvert={onConvert}
              />
            );
          }
          return (
            <>
              <SettingChips summary={summary} />
              <div className="flex min-h-0 flex-1">
                {summary.total > 0 ? <ChangeList summary={summary} /> : null}
                <div className="flex min-w-0 flex-1 flex-col">
                  {layout === "split" ? (
                    <div className="grid h-7 flex-none grid-cols-2 items-center border-b border-border text-xs">
                      <Caption kind={left.startsWith("NFS") ? "nfs" : "git"} label={left} />
                      <Caption kind={right.startsWith("NFS") ? "nfs" : "git"} label={right} />
                    </div>
                  ) : null}
                  <DiffView
                    ref={diff}
                    label={`Diff of ${name}`}
                    hunks={cmp.hunks}
                    mode={layout}
                    onActiveChange={(index, total) => setActive({ index, total })}
                    {...(folds === null ? {} : { getLines: folds.getLines, oldTotal: folds.oldTotal })}
                    className="min-h-0 flex-1"
                  />
                </div>
              </div>
            </>
          );
        }}
      </QueryBoundaryView>
    </div>
  );
}
