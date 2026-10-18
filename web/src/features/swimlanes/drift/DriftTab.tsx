import { useMemo, useState } from "react";
import { DriftState, type FileEntry, type SwimlaneDetail } from "@/api/generated/model";
import { DRIFT_VARIANT } from "@/components/app/drift";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { VirtualList } from "@/components/app/virtual/VirtualList";
import { Check, ChevronDown, ChevronRight, CircleHelp, Lock } from "@/icons";
import { swimlaneDriftQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { AppLink } from "@/routes/AppLink";
import { baselineView } from "../baseline";
import { groupDrift, rowsOf, type DriftRow } from "./group";

// The Drift tab: the files that are not in sync, grouped by state, most urgent first. Without a baseline the engine cannot say
// which files differ (every file is unknown), so a swimlane that is waiting for adoption says so instead of listing hundreds of
// "unknown" files. Who changed a file, how sure we are and how serious it is are task T5's: this tab leaves their place free.

const MEANING: Record<DriftState, string> = {
  in_sync: "NFS, Git and the baseline agree.",
  git_ahead: "Git has a newer version than the baseline. NFS has not caught up.",
  nfs_ahead: "NFS has a newer version than the baseline. Git does not have it yet.",
  conflict: "NFS and Git both changed since the baseline, and they differ.",
  unknown: "There is no baseline, and NFS differs from Git.",
  untracked: "On NFS only: Git has no such file and there is no baseline.",
  intentional_divergence: "Marked as intentional: NFS and Git are left to differ.",
};

const HEADER = 32;
const FILE = 56;

function FileRow({ lane, file }: { lane: string; file: FileEntry }) {
  return (
    <div className="flex h-full flex-col justify-center gap-0.5 border-b border-border py-1 pr-4 pl-9">
      <div className="flex items-center gap-2">
        <SafeText text={file.path} className="min-w-0 flex-1 truncate font-mono text-xs font-medium" />
        {file.tenant != null ? (
          <span className="inline-flex h-5 items-center rounded-sm border border-input px-1.5 font-mono text-xs">
            <SafeText text={file.tenant} />
          </span>
        ) : null}
        {file.content_withheld ? <span className="text-xs text-muted-foreground">Content withheld</span> : null}
      </div>
      <div className="flex items-center gap-3 text-xs">
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{MEANING[file.drift]}</span>
        <span className="flex shrink-0 gap-3 font-medium">
          <AppLink route="files" params={{ id: lane }} search={{ path: file.path }}>
            Open file
          </AppLink>
          {file.content_withheld ? null : (
            <AppLink route="files" params={{ id: lane }} search={{ path: file.path, diff: "git" }}>
              View diff
            </AppLink>
          )}
        </span>
      </div>
    </div>
  );
}

function List({ lane, files, truncated }: { lane: string; files: FileEntry[]; truncated: boolean }) {
  const [closed, setClosed] = useState<ReadonlySet<DriftState>>(() => new Set());
  const groups = useMemo(() => groupDrift(files), [files]);
  const rows = useMemo<DriftRow[]>(() => rowsOf(groups, closed), [groups, closed]);
  const total = groups.reduce((sum, g) => sum + g.files.length, 0);
  if (rows.length === 0) {
    return (
      <EmptyState icon={Check} title="Everything is in sync" className="flex-1">
        No file differs between NFS, Git and the baseline.
      </EmptyState>
    );
  }
  return (
    <>
      {truncated ? <TruncationBanner shown={files.length} noun="files" /> : null}
      <div className="flex h-8 flex-none items-center gap-3 border-b border-border bg-muted px-4 text-xs text-muted-foreground">
        <span>Files not in sync, grouped by state</span>
        <span className="ml-auto tabular-nums">{total} {total === 1 ? "file" : "files"}</span>
      </div>
      <VirtualList
        label="Files not in sync"
        role="list"
        className="flex-1"
        count={rows.length}
        rowHeight={(i) => (rows[i]?.kind === "group" ? HEADER : FILE)}
        getRowKey={(i) => {
          const row = rows[i];
          return row === undefined ? i : row.kind === "group" ? `group:${row.state}` : `file:${row.file.path}`;
        }}
        renderRow={(i) => {
          const row = rows[i];
          if (row === undefined) return null;
          if (row.kind === "file") return <FileRow lane={lane} file={row.file} />;
          return (
            <button
              type="button"
              aria-expanded={row.open}
              onClick={() => {
                setClosed((current) => {
                  const next = new Set(current);
                  if (!next.delete(row.state)) next.add(row.state);
                  return next;
                });
              }}
              className="flex h-full w-full cursor-pointer items-center gap-2 border-0 border-b border-border bg-transparent px-4 text-left"
            >
              {row.open ? <ChevronDown aria-hidden="true" className="size-3 text-muted-foreground" /> : <ChevronRight aria-hidden="true" className="size-3 text-muted-foreground" />}
              <StatusBadge variant={DRIFT_VARIANT[row.state]} />
              <span className="text-muted-foreground tabular-nums">{row.count}</span>
            </button>
          );
        }}
      />
    </>
  );
}

function Loaded({ lane }: { lane: SwimlaneDetail }) {
  const pages = useCursorPages({ ...swimlaneDriftQuery(lane.id, null), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  return (
    <QueryBoundaryView
      state={state}
      noun="drift"
      source="the drift list"
      className="flex min-h-0 flex-1 flex-col"
      empty={
        <EmptyState icon={Check} title="Everything is in sync" className="flex-1">
          No file differs between NFS, Git and the baseline.
        </EmptyState>
      }
    >
      {(files) => <List lane={lane.id} files={files} truncated={pages.truncated} />}
    </QueryBoundaryView>
  );
}

export function DriftTab({ lane }: { lane: SwimlaneDetail }) {
  const baseline = baselineView(lane.baseline).kind;
  if (baseline === "unconfirmed" || baseline === "in_progress") {
    return (
      <EmptyState icon={baseline === "unconfirmed" ? CircleHelp : Lock} title="Drift is unknown without a baseline" className="flex-1">
        Lanekeeper can't say which files differ until an admin confirms what <SafeText text={lane.display_name} className="font-mono" /> is running.
      </EmptyState>
    );
  }
  return <Loaded lane={lane} />;
}
