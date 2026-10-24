import { useQueryClient } from "@tanstack/react-query";
import { useMemo, useState, type ReactNode } from "react";
import type { SwimlaneSummary, TreeCompareEntry } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { TreeView, type TreeNode } from "@/components/app/virtual/TreeView";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { ArrowLeftRight, CircleCheck, File, FileCode, FileImage, Folder, FolderOpen, GitBranch, Layers, Minus, Plus, RefreshCw, Server, Tag } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { compareKeys } from "@/lib/query/keys";
import { cursorPagesSource, useQueryState, type QueryState } from "@/lib/query/use-query-state";
import { formatClock, formatRelative } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useNow } from "@/live/use-now";
import { isOffline, laneColumn, type LaneColumn } from "../grid/lanes";
import type { CompareView } from "../model";
import { OfflineLanesBanner } from "../OfflineLanesBanner";
import { treeComparisonQuery } from "../queries";
import { FilePanel, type SideNote } from "./FilePanel";
import { SidePicker } from "./SidePicker";
import { sidesOf, swapped, withLeftLane, withRight, withTarget, type Target } from "./sides";
import { StateChip } from "./StateChip";
import { buildTree, countStates, filterEntries, flattenIds, type Filter, type TreeNodeData } from "./tree";

// "Whole swimlane": every logical file of one swimlane against another swimlane, a Git branch or a Git tag. The server walks the
// two trees and skips the subtrees that are equal, so the page lists only the files that differ, as a tree; choosing one shows
// how the two sides differ (src/features/compare/lanes/FilePanel.tsx).

const MAX_AUTO_OPEN_DIRS = 200;

type Props = {
  view: CompareView;
  onView: (next: CompareView) => void;
  swimlanes: QueryState<SwimlaneSummary[]>;
};

const TARGETS: ReadonlyArray<{ value: Target; label: string; Icon: typeof Server }> = [
  { value: "lane", label: "Swimlane", Icon: Server },
  { value: "branch", label: "Git branch", Icon: GitBranch },
  { value: "tag", label: "Git tag", Icon: Tag },
];

export function LanesScreen({ view, onView, swimlanes }: Props) {
  const data = swimlanes.data;
  const all = useMemo<readonly SwimlaneSummary[]>(() => data ?? [], [data]);
  const pair = { left: view.left, right: view.right };
  const derived = sidesOf(pair);
  // With no right side yet the kind of it cannot be read from the references: it is what the person chose last.
  const [chosen, setChosen] = useState<Target | null>(null);
  const target = derived.rightName !== null ? derived.target : (chosen ?? derived.target);
  const sides = { ...derived, target };
  const [leftOpen, setLeftOpen] = useState(false);
  const set = (next: { left: string | null; right: string | null }) => {
    onView({ ...view, mode: "lanes", left: next.left, right: next.right, file: null });
  };
  const swap = swapped(pair);
  const ready = view.left !== null && view.right !== null;
  const leftLane = sides.leftLane === null ? null : laneColumn(sides.leftLane, all.find((l) => l.id === sides.leftLane));
  const rightLane = sides.target === "lane" && sides.rightName !== null ? laneColumn(sides.rightName, all.find((l) => l.id === sides.rightName)) : null;
  const offline = [leftLane, rightLane].filter((lane): lane is LaneColumn => lane !== null && isOffline(lane));

  return (
    <div className="mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
      <div role="group" aria-label="Sides of the comparison" className="flex min-h-10 flex-none flex-wrap items-center gap-2 border-b border-border bg-muted px-2 py-1.5">
        <SidePicker side="Left" target="lane" value={sides.leftLane} lanes={all} onChoose={(name) => set(withLeftLane(pair, name))} open={leftOpen} onOpenChange={setLeftOpen} />
        <Button
          variant="ghost"
          size="icon"
          aria-label="Swap sides"
          title="Swap sides"
          disabled={swap === null}
          onClick={() => {
            if (swap !== null) set(swap);
          }}
        >
          <ArrowLeftRight />
        </Button>
        <ToggleGroup
          type="single"
          value={sides.target}
          onValueChange={(next) => {
            if (next !== "lane" && next !== "branch" && next !== "tag") return;
            setChosen(next);
            set(withTarget(pair, next));
          }}
          aria-label="Compare against"
        >
          {TARGETS.map(({ value, label, Icon }) => (
            <ToggleGroupItem key={value} value={value}>
              <Icon aria-hidden="true" />
              {label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <SidePicker side="Right" target={sides.target} value={sides.rightName} lanes={all} onChoose={(name) => set(withRight(pair, name, sides.target))} />
      </div>
      {offline.length > 0 && ready ? <OfflineBanner lanes={offline} onRetry={() => swimlanes.refetch()} /> : null}
      {ready ? (
        <Comparison key={`${view.left}|${view.right}`} view={view} left={view.left ?? ""} right={view.right ?? ""} sides={sides} leftLane={leftLane} rightLane={rightLane} onView={onView} swimlanes={swimlanes} />
      ) : (
        <EmptyState
          icon={Layers}
          title="Choose two swimlanes to compare"
          action={
            <Button
              onClick={() => {
                setLeftOpen(true);
              }}
            >
              Choose swimlanes
            </Button>
          }
        >
          Pick a left and a right swimlane. Files that differ are listed here.
        </EmptyState>
      )}
    </div>
  );
}

function OfflineBanner({ lanes, onRetry }: { lanes: readonly LaneColumn[]; onRetry: () => void }) {
  const now = useNow(true, 30_000);
  const when = (lane: LaneColumn) => (lane.asOf === null ? "from its last scan" : `as of ${formatClock(lane.asOf)}, ${formatRelative(lane.asOf, now)}`);
  return (
    <OfflineLanesBanner lanes={lanes} onRetry={onRetry}>
      {lanes.map((lane) => `${lane.id}: its NFS files ${when(lane)}`).join("; ")}. Differences made since then are not included.
    </OfflineLanesBanner>
  );
}

type ComparisonProps = {
  view: CompareView;
  left: string;
  right: string;
  sides: ReturnType<typeof sidesOf>;
  leftLane: LaneColumn | null;
  rightLane: LaneColumn | null;
  onView: (next: CompareView) => void;
  swimlanes: QueryState<SwimlaneSummary[]>;
};

function Comparison({ view, left, right, sides, leftLane, rightLane, onView, swimlanes }: ComparisonProps) {
  const client = useQueryClient();
  const options = useMemo(() => treeComparisonQuery(left, right, ""), [left, right]);
  const pages = useCursorPages({ ...options, autoLoad: true, maxPages: 10 });
  const query = useQueryState(cursorPagesSource(pages), { isEmpty: (entries) => !pages.hasMore && entries.every((entry) => entry.state === "same") });
  const [filter, setFilter] = useState<Filter>("all");
  const clock = useNow(true, 30_000);
  // The count of identical subtrees the server skipped is on the pages, not in the entries.
  const skipped = client.getQueryData<{ pages: Array<{ skipped: number }> }>(compareKeys.tree(left, right, ""))?.pages[0]?.skipped ?? 0;
  const counts = countStates(pages.items);
  const leftName = sides.leftLane ?? left;
  const rightName = sides.rightName ?? right;
  const more = pages.hasMore ? "+" : "";

  const cards: ReadonlyArray<{ key: Filter | null; label: ReactNode; n: number; Icon: typeof CircleCheck; classes: string }> = [
    { key: null, label: "Identical directories", n: skipped, Icon: CircleCheck, classes: "border-success-border bg-success-bg text-success-fg" },
    { key: "different", label: "Differing logical files", n: counts.different, Icon: ArrowLeftRight, classes: "border-warning-border bg-diff-mod-bg text-diff-mod-fg" },
    { key: "left_only", label: <>Only in <SafeText text={leftName} /></>, n: counts.left_only, Icon: Minus, classes: "border-danger-border bg-diff-del-bg text-diff-del-fg" },
    { key: "right_only", label: <>Only in <SafeText text={rightName} /></>, n: counts.right_only, Icon: Plus, classes: "border-success-border bg-diff-add-bg text-diff-add-fg" },
  ];
  const loading = query.state === "loading";

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-none flex-wrap items-center gap-2 border-b border-border px-2 py-1.5">
        <div role="group" aria-label="Summary" className="flex flex-wrap items-stretch gap-2">
          {cards.map(({ key, label, n, Icon, classes }) => {
            const body = (
              <>
                <span aria-hidden="true" className={cn("inline-flex size-6 shrink-0 items-center justify-center rounded-sm border", classes)}>
                  <Icon className="size-3.5" />
                </span>
                {loading ? (
                  <span className="flex flex-col gap-1">
                    <Skeleton className="h-3 w-8" />
                    <Skeleton className="h-3 w-24" />
                  </span>
                ) : (
                  <span className="flex flex-col text-left">
                    <span className="text-base leading-5 font-semibold tabular-nums">
                      {n.toLocaleString("en-US")}
                      {key === null ? "" : more}
                    </span>
                    <span className="text-xs text-muted-foreground">{label}</span>
                  </span>
                )}
              </>
            );
            return key === null ? (
              <div key="identical" className="flex items-center gap-2 rounded-sm border border-border bg-card px-2 py-1">
                {body}
              </div>
            ) : (
              <button
                key={key}
                type="button"
                aria-pressed={filter === key}
                onClick={() => {
                  setFilter((current) => (current === key ? "all" : key));
                }}
                className="flex cursor-pointer items-center gap-2 rounded-sm border border-border bg-card px-2 py-1 hover:bg-accent aria-pressed:border-primary aria-pressed:bg-selection"
              >
                {body}
              </button>
            );
          })}
        </div>
        <span className="ml-auto inline-flex items-center gap-1.5 text-xs text-muted-foreground tabular-nums">
          {loading ? (
            <span role="status" className="inline-flex items-center gap-1.5 text-foreground">
              <Spinner />
              Reading directories…
            </span>
          ) : query.state === "ready" || query.state === "empty" ? (
            <span>
              Compared at {formatClock(pages.dataUpdatedAt)} · {counts.different + counts.left_only + counts.right_only}
              {more} files differ
            </span>
          ) : null}
        </span>
        <Button variant="ghost" size="sm" aria-label="Compare again" title="Compare again" onClick={() => void pages.refetch()}>
          <RefreshCw data-icon="inline-start" />
          Compare again
        </Button>
      </div>
      {pages.truncated ? <TruncationBanner shown={pages.items.length} noun="files" className="border-b" /> : null}
      <QueryBoundaryView
        state={query}
        noun="comparison"
        source="both swimlanes"
        now={clock}
        className="flex min-h-0 flex-1 flex-col"
        loading={<LoadingSplit />}
        empty={
          <EmptyState
            icon={CircleCheck}
            title={sides.target === "lane" ? `${leftName} and ${rightName} are identical` : `${leftName} matches ${sides.target === "branch" ? "Git" : "tag"} ${rightName}`}
            action={
              <Button variant="secondary" onClick={() => void pages.refetch()}>
                <RefreshCw data-icon="inline-start" />
                Compare again
              </Button>
            }
          >
            Every file matches, including which file each side reads (tenant or base). {skipped > 0 ? `${skipped.toLocaleString("en-US")} identical directories were skipped without reading their files. ` : ""}Compared at {formatClock(pages.dataUpdatedAt)}. Nothing to review.
          </EmptyState>
        }
      >
        {(entries) => <Split entries={entries} filter={filter} view={view} onView={onView} left={left} right={right} leftName={leftName} rightName={rightName} leftLane={leftLane} rightLane={rightLane} skipped={skipped} swimlanes={swimlanes} />}
      </QueryBoundaryView>
    </div>
  );
}

function LoadingSplit() {
  return (
    <div aria-busy="true" className="grid min-h-0 flex-1 grid-cols-[minmax(260px,380px)_1fr]">
      <div className="flex flex-col gap-2 border-r border-border p-3" role="status" aria-label="Loading directories">
        {Array.from({ length: 14 }, (_, i) => (
          <Skeleton key={i} className="h-3" style={{ width: `${90 + ((i * 31) % 130)}px`, marginLeft: i % 4 === 0 ? 0 : i % 4 === 1 ? 16 : 32 }} />
        ))}
        <p className="mt-2 text-xs text-muted-foreground">Reading directories…</p>
      </div>
      <div />
    </div>
  );
}

const fileIcon = (path: string) => (/\.(ya?ml|json|properties|xml)$/i.test(path) ? FileCode : /\.(png|jpe?g|gif|svg|webp|ico)$/i.test(path) ? FileImage : File);

function toNodes(nodes: readonly TreeNodeData[], leftName: string, rightName: string, openIds: ReadonlySet<string>): TreeNode[] {
  // Iterative, so a deep path cannot overflow the stack: children are built before their parents.
  const root: TreeNode[] = [];
  const stack: Array<{ list: readonly TreeNodeData[]; into: TreeNode[]; at: number }> = [{ list: nodes, into: root, at: 0 }];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (frame === undefined) break;
    const node = frame.list[frame.at];
    if (node === undefined) {
      stack.pop();
      continue;
    }
    frame.at += 1;
    if (node.kind === "file") {
      const Icon = fileIcon(node.path);
      frame.into.push({
        id: node.id,
        text: node.name,
        label: <SafeText text={node.name} className="font-mono text-xs" />,
        icon: <Icon aria-hidden="true" className="size-3.5" />,
        trailing: <StateChip state={node.state} leftName={leftName} rightName={rightName} />,
      });
    } else {
      const children: TreeNode[] = [];
      const Icon = openIds.has(node.id) ? FolderOpen : Folder;
      frame.into.push({
        id: node.id,
        text: node.name,
        label: <SafeText text={node.name} className="font-mono text-xs font-medium" />,
        icon: <Icon aria-hidden="true" className="size-3.5" />,
        trailing: (
          <span title={`${node.count} differing files`} className="inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-muted px-1 text-xs text-muted-foreground tabular-nums">
            {node.count}
          </span>
        ),
        children,
      });
      stack.push({ list: node.children, into: children, at: 0 });
    }
  }
  return root;
}

type SplitProps = {
  entries: readonly TreeCompareEntry[];
  filter: Filter;
  view: CompareView;
  onView: (next: CompareView) => void;
  left: string;
  right: string;
  leftName: string;
  rightName: string;
  leftLane: LaneColumn | null;
  rightLane: LaneColumn | null;
  skipped: number;
  swimlanes: QueryState<SwimlaneSummary[]>;
};

function Split({ entries, filter, view, onView, left, right, leftName, rightName, leftLane, rightLane, skipped }: SplitProps) {
  const shown = useMemo(() => filterEntries(entries, filter), [entries, filter]);
  const tree = useMemo(() => buildTree(shown), [shown]);
  const [open, setOpen] = useState<ReadonlySet<string> | null>(null);
  const ids = useMemo(() => flattenIds(tree), [tree]);
  const dirs = useMemo(() => ids.filter((id) => id.startsWith("d:")), [ids]);
  // Open every folder, unless there are very many: then only the first level.
  const defaultOpen = useMemo(() => new Set(dirs.length <= MAX_AUTO_OPEN_DIRS ? dirs : tree.filter((n) => n.kind === "dir").map((n) => n.id)), [dirs, tree]);
  const expanded = open ?? defaultOpen;
  const nodes = useMemo(() => toNodes(tree, leftName, rightName, expanded), [tree, leftName, rightName, expanded]);

  const files = useMemo(() => new Map(shown.map((entry) => [`f:${entry.file}`, entry])), [shown]);
  // The file in the address if there is one, else the first file of the tree (the person has not chosen yet).
  const firstFile = ids.find((id) => id.startsWith("f:"));
  const selectedId = view.file !== null ? `f:${view.file}` : (firstFile ?? null);
  const selected = selectedId === null ? undefined : files.get(selectedId);
  const selectedPath = view.file ?? (firstFile === undefined ? null : firstFile.slice(2));

  const note = (ref: string, lane: LaneColumn | null): SideNote => (lane !== null && isOffline(lane) ? { ref, offlineAsOf: lane.asOf } : { ref });

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[minmax(260px,380px)_1fr]">
      <div className="flex min-h-0 flex-col border-r border-border">
        <div className="flex flex-none items-center gap-2 border-b border-border px-3 py-1.5">
          <span className="text-xs font-medium text-muted-foreground">Differing directories</span>
          <span className="text-xs text-muted-foreground tabular-nums">
            {shown.length} {shown.length === 1 ? "file" : "files"} in {dirs.length} {dirs.length === 1 ? "directory" : "directories"}
          </span>
          <span className="ml-auto inline-flex gap-1">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setOpen(new Set(dirs));
              }}
            >
              Expand
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setOpen(new Set());
              }}
            >
              Collapse
            </Button>
          </span>
        </div>
        <div className="min-h-0 flex-1">
          {nodes.length === 0 ? (
            <p className="p-4 text-sm text-muted-foreground">No file in this group.</p>
          ) : (
            <TreeView
              label="Differing files"
              nodes={nodes}
              expanded={expanded}
              onExpandedChange={(next) => {
                setOpen(next);
              }}
              selectedId={selectedId}
              onSelect={(id) => {
                if (id.startsWith("f:")) onView({ ...view, file: id.slice(2) });
                else setOpen((current) => {
                  const next = new Set(current ?? expanded);
                  if (!next.delete(id)) next.add(id);
                  return next;
                });
              }}
              className="h-full"
            />
          )}
        </div>
        <div className="flex flex-none items-center gap-2 border-t border-border px-3 py-1.5 text-xs text-muted-foreground">
          <CircleCheck aria-hidden="true" className="size-3.5 text-success-fg" />
          <span>{skipped.toLocaleString("en-US")} identical directories skipped</span>
        </div>
      </div>
      <div className="flex min-h-0 min-w-0 flex-col">
        {selectedPath === null ? (
          <EmptyState icon={File} title="Select a file">
            Choose a file in the tree to see how the two sides differ.
          </EmptyState>
        ) : (
          <FilePanel key={selectedPath} left={note(left, leftLane)} right={note(right, rightLane)} file={selectedPath} state={selected?.state ?? null} />
        )}
      </div>
    </div>
  );
}
