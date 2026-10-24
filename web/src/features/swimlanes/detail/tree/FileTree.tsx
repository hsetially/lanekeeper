import { useCallback, useEffect, useId, useMemo, useRef, useState, type Ref } from "react";
import { DriftState, FileClass, type FileEntry } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { EmptyState } from "@/components/app/EmptyState";
import { ErrorState } from "@/components/app/ErrorState";
import { SafeText } from "@/components/app/SafeText";
import { STATUS_VARIANTS, StatusBadge } from "@/components/app/StatusBadge";
import { DRIFT_VARIANT } from "@/components/app/drift";
import { TreeView, type TreeNode } from "@/components/app/virtual/TreeView";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { SearchInput } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { fileTreeQuery } from "@/features/file/queries";
import { TreePickup, pickupRollups, type PickupRollup } from "@/features/pickup";
import { File as FileIcon, Folder, Layers, Plus } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cn } from "@/lib/utils";
import { openInNewTab, useNavigateTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import { buildTree, type DataNode, type Folders } from "./build";

// The files of the swimlane, on the left of its screens: the folders it has, opened one at a time (the server lists a folder per
// request), with the drift of each file and, when the person asks for them, the logical files (a base file and the tenant file
// that replaces it as one row). A file opens in the file view; a folder opens in place. The filter looks at the names of what has
// been loaded, because the server has no search by name here: it says so when it finds nothing.

type FolderState = { entries: readonly FileEntry[]; hasMore: boolean; status: "pending" | "error" | "success"; error: ApiProblem | null; loadMore: () => Promise<void>; retry: () => Promise<void> };

/** Loads one folder and reports it to the tree. It draws nothing: an open folder is a mounted loader, a closed one is not. */
function FolderLoader({ id, prefix, drift, report }: { id: string; prefix: string; drift: DriftState | null; report: (prefix: string, state: FolderState) => void }) {
  const pages = useCursorPages(fileTreeQuery(id, prefix, drift));
  const { items, hasMore, status, error } = pages;
  // `loadMore` and `refetch` are new functions on every render of the list, so what is reported is a pair that never changes and
  // calls the latest: reporting a new pair each time would report again, and re-render the tree, forever.
  const latest = useRef(pages);
  useEffect(() => {
    latest.current = pages;
  });
  const actions = useMemo(() => ({ loadMore: () => latest.current.loadMore(), retry: () => latest.current.refetch() }), []);
  useEffect(() => {
    report(prefix, { entries: items, hasMore, status, error, ...actions });
  }, [report, prefix, items, hasMore, status, error, actions]);
  return null;
}

const DRIFT_CHOICES = [DriftState.conflict, DriftState.nfs_ahead, DriftState.git_ahead, DriftState.unknown, DriftState.untracked, DriftState.intentional_divergence, DriftState.in_sync] as const;
const CLASS_CHOICES: ReadonlyArray<[FileClass, string]> = [
  [FileClass.structured, "Structured"],
  [FileClass.text, "Text"],
  [FileClass.binary, "Binary"],
  [FileClass.denied, "Denied"],
];
const ALL = "__all";

function FilterButton({ label, shown, checked, onValue, children }: { label: string; shown: string | null; checked: string; onValue: (value: string) => void; children: React.ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className={cn("gap-1 px-1.5 text-xs [&_svg:not([class*='size-'])]:size-3", "border-dashed border-input bg-transparent font-normal hover:bg-accent", shown !== null && "border-solid bg-background")}>
          {shown === null ? <Plus data-icon="inline-start" /> : null}
          {shown === null ? label : `${label}: ${shown}`}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuRadioGroup value={checked} onValueChange={onValue}>
          {children}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Row({ node }: { node: DataNode }) {
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <SafeText text={node.isDir ? `${node.name}/` : node.name} className={cn("min-w-0 truncate font-mono text-xs", node.isDir ? "font-medium" : "font-normal")} />
    </span>
  );
}

function Trailing({ node, rollup }: { node: DataNode; rollup: PickupRollup | undefined }) {
  return (
    <span className="flex items-center gap-1.5">
      {node.usesTenantFile.length > 0 ? (
        <span title={`Reads ${node.usesTenantFile.join(", ")} beside the base file`} className="inline-flex h-5 items-center gap-1 rounded-sm border border-input bg-background px-1.5 text-xs whitespace-nowrap">
          <Layers aria-hidden="true" className="size-3" />
          Uses tenant file
        </span>
      ) : null}
      {!node.isDir && node.more === null && node.drift !== DriftState.in_sync ? <StatusBadge variant={DRIFT_VARIANT[node.drift]} /> : null}
      <TreePickup node={node} rollup={rollup} />
    </span>
  );
}

function toTreeNode(node: DataNode, rollups: ReadonlyMap<string, PickupRollup>): TreeNode {
  if (node.more !== null) return { id: node.id, label: <span className="text-muted-foreground">Load more…</span> };
  return {
    id: node.id,
    label: <Row node={node} />,
    text: node.name,
    lazy: node.lazy,
    icon: node.isDir ? <Folder aria-hidden="true" className="size-3.5" /> : <FileIcon aria-hidden="true" className="size-3.5" />,
    trailing: <Trailing node={node} rollup={rollups.get(node.id)} />,
    children: node.children.map((child) => toTreeNode(child, rollups)),
  };
}

function find(nodes: readonly DataNode[], id: string): DataNode | undefined {
  for (const node of nodes) {
    if (node.id === id) return node;
    const inner = find(node.children, id);
    if (inner) return inner;
  }
  return undefined;
}

export function FileTree({ id, name, filterRef }: { id: string; name: string; filterRef: Ref<HTMLInputElement> }) {
  const navigate = useNavigateTo();
  const switchId = useId();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const [logical, setLogical] = useState(false);
  const [filter, setFilter] = useState("");
  const [drift, setDrift] = useState<DriftState | null>(null);
  const [fileClass, setFileClass] = useState<FileClass | null>(null);
  const [data, setData] = useState<ReadonlyMap<string, FolderState>>(() => new Map());

  const report = useCallback((prefix: string, state: FolderState) => {
    setData((current) => new Map(current).set(prefix, state));
  }, []);
  // A different drift filter is a different set of pages for every folder.
  useEffect(() => {
    setData(new Map());
  }, [drift, id]);

  const folders = useMemo<Folders>(() => new Map([...data].filter(([, s]) => s.status === "success").map(([prefix, s]) => [prefix, { entries: s.entries, hasMore: s.hasMore }])), [data]);
  const tree = useMemo(() => buildTree(folders, { logical, filter, fileClass }), [folders, logical, filter, fileClass]);
  const nodes = useMemo(() => {
    const rollups = pickupRollups(tree);
    return tree.map((node) => toTreeNode(node, rollups));
  }, [tree]);
  const root = data.get("");
  const open = ["", ...expanded];

  const choose = (nodeId: string) => {
    const node = find(tree, nodeId);
    if (node === undefined) return;
    if (node.more !== null) {
      void data.get(node.more)?.loadMore();
      return;
    }
    if (node.isDir) {
      setExpanded((current) => {
        const next = new Set(current);
        if (!next.delete(node.id)) next.add(node.id);
        return next;
      });
      return;
    }
    navigate(linkTo("files", { id }, { path: node.entry?.path ?? node.id }));
  };

  // Control or Command and Enter on a file opens it in a new tab; a folder or a "Load more" row has nothing to open.
  const openFileInNewTab = (nodeId: string) => {
    const node = find(tree, nodeId);
    if (node === undefined || node.more !== null || node.isDir) return;
    openInNewTab(linkTo("files", { id }, { path: node.entry?.path ?? node.id }));
  };

  return (
    <section aria-label="Files" className="flex h-full min-h-0 flex-col bg-muted">
      {open.map((prefix) => (
        <FolderLoader key={`${prefix}|${drift ?? ""}`} id={id} prefix={prefix} drift={drift} report={report} />
      ))}
      <div className="flex flex-none flex-col gap-2 border-b border-border p-2">
        <SearchInput
          ref={filterRef}
          aria-label="Filter files"
          placeholder="Filter files…"
          kbd="f"
          value={filter}
          onChange={(event) => {
            setFilter(event.target.value);
          }}
        />
        <div className="flex flex-wrap items-center gap-1.5">
          <FilterButton
            label="Drift state"
            shown={drift === null ? null : STATUS_VARIANTS[DRIFT_VARIANT[drift]].label}
            checked={drift ?? ALL}
            onValue={(value) => {
              setDrift(DRIFT_CHOICES.find((state) => state === value) ?? null);
            }}
          >
            <DropdownMenuRadioItem value={ALL}>All states</DropdownMenuRadioItem>
            {DRIFT_CHOICES.map((state) => (
              <DropdownMenuRadioItem key={state} value={state}>
                {STATUS_VARIANTS[DRIFT_VARIANT[state]].label}
              </DropdownMenuRadioItem>
            ))}
          </FilterButton>
          <FilterButton
            label="File class"
            shown={fileClass === null ? null : (CLASS_CHOICES.find(([c]) => c === fileClass)?.[1] ?? null)}
            checked={fileClass ?? ALL}
            onValue={(value) => {
              setFileClass(CLASS_CHOICES.find(([c]) => c === value)?.[0] ?? null);
            }}
          >
            <DropdownMenuRadioItem value={ALL}>All classes</DropdownMenuRadioItem>
            {CLASS_CHOICES.map(([value, label]) => (
              <DropdownMenuRadioItem key={value} value={value}>
                {label}
              </DropdownMenuRadioItem>
            ))}
          </FilterButton>
          <label htmlFor={switchId} className="ml-auto inline-flex cursor-pointer items-center gap-2 text-xs">
            <Switch id={switchId} checked={logical} onCheckedChange={setLogical} />
            Show logical files
          </label>
        </div>
      </div>
      {root === undefined || root.status === "pending" ? (
        <div role="status" aria-busy="true" aria-label="Loading files" className="flex flex-col gap-1.5 p-3">
          {[0, 1, 2, 3, 4, 5].map((i) => (
            <Skeleton key={i} className="h-4" style={{ width: `${70 - i * 6}%` }} />
          ))}
        </div>
      ) : root.status === "error" ? (
        <ErrorState title="Couldn't load the files" {...(root.error ? { problem: root.error } : {})} onRetry={() => void root.retry()} className="py-6" />
      ) : root.entries.length === 0 ? (
        <EmptyState title={drift === null ? "No files" : "No file in this state"} className="py-6">
          {drift === null ? "This swimlane has no files yet." : "Choose another state, or all states."}
        </EmptyState>
      ) : nodes.length === 0 ? (
        <EmptyState title="No file matches" className="py-6">
          The filter looks at the names of the folders you have opened.
        </EmptyState>
      ) : (
        <TreeView label={`Files in ${name}`} nodes={nodes} expanded={expanded} onExpandedChange={setExpanded} onSelect={choose} onOpenInNewTab={openFileInNewTab} className="min-h-0 flex-1 py-1" />
      )}
    </section>
  );
}

