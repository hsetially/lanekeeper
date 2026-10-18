import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { Comparison, CompareState } from "@/api/generated/model";
import { Banner } from "@/components/app/Banner";
import { CopyButton } from "@/components/app/CopyButton";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { DiffView } from "@/components/app/diff/DiffView";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { CircleCheck, File, FileCode, FileImage, GitBranch, Lock, Server, Tag, WifiOff } from "@/icons";
import { useShortcut } from "@/keyboard";
import { useQueryState } from "@/lib/query/use-query-state";
import { formatClock } from "@/lib/time";
import { describeRef } from "../model";
import { compareFileQuery } from "../queries";
import { StateChip } from "./StateChip";

// The file chosen in the tree: how the two sides differ, as the server computed it. A text file is a split or unified diff of
// the server's hunks; a binary file is compared by hash, as the server says; a file that is on one side only is shown whole.
// The comparison is keyed by the two references and the file, never by a hash learned earlier (a reference moves), and the
// fetch layer asks again with the ETag it kept, so a file that changed since the last answer gives the new answer.

export type SideNote = {
  /** The reference of the side (`effective:sitb`, `git:tenant@sit7`). */
  ref: string;
  /** The side's agent is offline: when it was last heard from, or null if never. Undefined: the agent is fine. */
  offlineAsOf?: number | null;
};

type Props = { left: SideNote; right: SideNote; file: string; state: CompareState | null };

/** What a side is, in the design's words: where it is read (NFS, Git) and from what (the swimlane, the branch or tag). */
function sideWords(ref: string): { kind: string; where: string; Icon: typeof Server } {
  const side = describeRef(ref);
  if (side.kind === "git") return { kind: "Git", where: `${side.repo === "tenant" ? "tenant repository" : "base repository"} · ${side.name}`, Icon: side.repo === "tenant" ? GitBranch : Tag };
  return { kind: side.source === "Effective" ? "Effective" : side.source, where: side.name, Icon: Server };
}

function iconOf(file: string): typeof FileCode {
  if (/\.(ya?ml|json|properties|xml)$/i.test(file)) return FileCode;
  if (/\.(png|jpe?g|gif|svg|webp|ico)$/i.test(file)) return FileImage;
  return File;
}

function stat(comparison: Comparison): string {
  if (comparison.class === "binary") return "Binary";
  if (comparison.class === "denied") return "Name, size and hash only";
  let added = 0;
  let removed = 0;
  for (const hunk of comparison.hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "added") added += 1;
      else if (line.kind === "removed") removed += 1;
    }
  }
  return `${added} added · ${removed} removed`;
}

function Side({ note }: { note: SideNote }) {
  const { kind, where, Icon } = sideWords(note.ref);
  const offline = note.offlineAsOf !== undefined;
  return (
    <div className="flex min-w-0 items-center gap-2 px-3 py-1.5 first:border-r first:border-border">
      <Icon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="text-xs font-medium">{kind}</span>
      <SafeText text={where} className="min-w-0 truncate font-mono text-xs" />
      {offline ? (
        <span role="status" className="inline-flex h-[18px] shrink-0 items-center gap-1 rounded-full border border-dashed border-input px-1.5 text-xs text-muted-foreground">
          <WifiOff aria-hidden="true" className="size-3" />
          {note.offlineAsOf === null || note.offlineAsOf === undefined ? "no scan on record" : `as of ${formatClock(note.offlineAsOf)}`}
        </span>
      ) : null}
    </div>
  );
}

function Hash({ label, hash, size }: { label: string; hash: string | null; size: number | null }) {
  return (
    <>
      <span className="text-muted-foreground">{label}</span>
      <span className="flex min-w-0 items-center gap-1 font-mono text-xs">
        {hash === null ? (
          <span className="text-muted-foreground">not on this side</span>
        ) : (
          <>
            <span title={hash}>{hash.slice(0, 12)}</span>
            <CopyButton value={hash} what="hash" />
          </>
        )}
        {size === null ? null : <span className="text-muted-foreground">· {size.toLocaleString("en-US")} bytes</span>}
      </span>
    </>
  );
}

export function FilePanel({ left, right, file, state }: Props) {
  const query = useQuery(compareFileQuery(left.ref, right.ref, file, null));
  const view = useQueryState(query);
  const [mode, setMode] = useState<"split" | "unified">("split");
  useShortcut("diff.toggle-split-and-unified", () => {
    setMode((current) => (current === "split" ? "unified" : "split"));
  });
  const Icon = iconOf(file);
  const sideNames = [sideWords(left.ref).where, sideWords(right.ref).where];
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex flex-none flex-wrap items-center gap-2 border-b border-border px-3 py-2">
        <Icon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        <h2 className="min-w-0 truncate font-mono text-sm font-semibold">
          <SafeText text={file} />
        </h2>
        {state === null || state === "same" ? null : <StateChip state={state} leftName={sideNames[0] ?? ""} rightName={sideNames[1] ?? ""} />}
        <span className="text-xs text-muted-foreground tabular-nums">{view.data === undefined ? "" : stat(view.data)}</span>
        <ToggleGroup
          type="single"
          value={mode}
          onValueChange={(next) => {
            if (next === "split" || next === "unified") setMode(next);
          }}
          aria-label="Diff view"
          className="ml-auto"
        >
          <ToggleGroupItem value="split" aria-label="Split view">
            Split
          </ToggleGroupItem>
          <ToggleGroupItem value="unified" aria-label="Unified view">
            Unified
          </ToggleGroupItem>
        </ToggleGroup>
      </div>
      <div className="grid flex-none grid-cols-2 border-b border-border bg-muted">
        <Side note={left} />
        <Side note={right} />
      </div>
      <QueryBoundaryView state={view} noun="comparison" source="both swimlanes" className="flex min-h-0 flex-1 flex-col">
        {(comparison) => <PanelBody comparison={comparison} mode={mode} />}
      </QueryBoundaryView>
    </div>
  );
}

function PanelBody({ comparison, mode }: { comparison: Comparison; mode: "split" | "unified" }) {
  const hunks = useMemo(() => comparison.hunks, [comparison.hunks]);
  if (comparison.class === "denied") {
    return (
      <EmptyState icon={Lock} title="Content withheld">
        This file is known by name, size and hash only. Its content is not stored or shown, so the two sides cannot be compared line by line.
      </EmptyState>
    );
  }
  if (comparison.class === "binary") {
    const b = comparison.binary;
    return (
      <div className="flex flex-col items-start gap-2 p-6">
        <FileImage aria-hidden="true" className="size-5 text-muted-foreground" />
        <strong className="text-base font-semibold">{comparison.identical ? "Binary files are identical" : "Binary files differ"}</strong>
        <span className="text-muted-foreground">Binary content is compared by hash, not line by line.</span>
        {b === null ? null : (
          <div className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
            <Hash label="Left" hash={b.left} size={b.left_size} />
            <Hash label="Right" hash={b.right} size={b.right_size} />
          </div>
        )}
      </div>
    );
  }
  if (comparison.identical) {
    return (
      <EmptyState icon={CircleCheck} title="These two files are identical">
        Nothing differs between the two sides for this file.
      </EmptyState>
    );
  }
  return (
    <>
      {comparison.eol_only ? (
        <Banner variant="info" title="Line endings only">
          The two files differ only in their line endings.
        </Banner>
      ) : null}
      <DiffView label="Differences in the file" hunks={hunks} mode={mode} className="min-h-0 flex-1" empty="No lines differ." />
    </>
  );
}
