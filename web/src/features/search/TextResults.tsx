import { useCallback, useMemo, type MouseEvent } from "react";
import { SafeText } from "@/components/app/SafeText";
import { TierChip } from "@/components/app/TierBadge";
import { TreeView, type TreeNode } from "@/components/app/virtual/TreeView";
import { TenantSet } from "@/features/shell/Tenants";
import { FileCode } from "@/icons";
import { AppLink, openInNewTab, useNavigateTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import type { LaneInfo } from "./lanes";
import { fileParts, pluralise, windowedHit, type LaneGroup } from "./results";

// The hits of a text search as the design's tree (frames 1a to 1f): a swimlane, its files, and under each file the lines that
// matched, with the match marked. It is the shared TreeView, so the arrows, Home and End, type-ahead and the virtualisation
// (a search can return 500 hits) are the shared ones. A hit is a real link to its file at its line; Enter on it, a click and
// "open in a new tab" all go to the same address.

function HitLabel({ group, file, hit }: { group: LaneGroup; file: LaneGroup["files"][number]; hit: LaneGroup["files"][number]["hits"][number]["hit"] }) {
  const { segments, cutBefore, cutAfter } = windowedHit(hit.text, hit.ranges);
  const { name } = fileParts(file.path);
  return (
    <AppLink
      route="files"
      params={{ id: group.swimlane }}
      search={{ path: hit.path, line: String(hit.line) }}
      aria-label={`${group.swimlane} ${name} line ${hit.line}`}
      tabIndex={-1}
      // The row is the tree item; a click on the link must not also select the row, which would open the same address twice.
      onClick={(event: MouseEvent) => {
        event.stopPropagation();
      }}
      className="ml-7 grid grid-cols-[48px_minmax(0,1fr)] items-center gap-3 text-foreground no-underline hover:text-foreground"
    >
      <span className="text-right font-mono text-xs tabular-nums text-muted-foreground">{hit.line}</span>
      <span className="truncate whitespace-pre font-mono text-xs">
        {cutBefore ? "…" : null}
        {segments.map((segment, i) =>
          segment.mark ? (
            <mark key={i} className="rounded-[2px] bg-selection px-0 font-semibold text-foreground shadow-[inset_0_0_0_1px_var(--info-border)]">
              <SafeText text={segment.text} />
            </mark>
          ) : (
            <SafeText key={i} text={segment.text} />
          ),
        )}
        {cutAfter ? "…" : null}
      </span>
    </AppLink>
  );
}

function KindChip({ kind }: { kind: "tenant" | "base" }) {
  return (
    <span
      className={
        kind === "tenant"
          ? "inline-flex h-[18px] items-center rounded-sm border border-info-border bg-info-bg px-1.5 text-xs font-medium text-info-fg"
          : "inline-flex h-[18px] items-center rounded-sm border border-border px-1.5 text-xs font-medium text-muted-foreground"
      }
    >
      {kind}
    </span>
  );
}

export function buildNodes(groups: readonly LaneGroup[], lanes: ReadonlyMap<string, LaneInfo>): TreeNode[] {
  return groups.map((group) => {
    const info = lanes.get(group.swimlane);
    return {
      id: group.id,
      text: group.swimlane,
      label: (
        <span className="flex items-center gap-2">
          {info?.tier ? <TierChip tier={info.tier} /> : null}
          <SafeText text={group.swimlane} className="font-mono text-xs font-medium" />
          {info ? <TenantSet tenants={info.tenants} /> : null}
        </span>
      ),
      trailing: <span className="text-xs tabular-nums text-muted-foreground">{`${pluralise(group.hits, "match", "matches")} · ${pluralise(group.files.length, "file", "files")}`}</span>,
      children: group.files.map((file) => {
        const { dir, name } = fileParts(file.path);
        return {
          id: file.id,
          text: name,
          icon: <FileCode aria-hidden="true" className="size-3.5" />,
          label: (
            <span className="font-mono text-xs">
              {dir === "" ? null : (
                <span className="text-muted-foreground">
                  <SafeText text={dir} />/
                </span>
              )}
              <span className="font-medium">
                <SafeText text={name} />
              </span>
            </span>
          ),
          trailing: (
            <span className="flex items-center gap-2">
              <KindChip kind={file.kind} />
              <span className="text-xs tabular-nums text-muted-foreground">{file.hits.length}</span>
            </span>
          ),
          children: file.hits.map(({ id, hit }) => ({ id, label: <HitLabel group={group} file={file} hit={hit} /> })),
        };
      }),
    };
  });
}

type Props = {
  groups: readonly LaneGroup[];
  lanes: ReadonlyMap<string, LaneInfo>;
  /** Swimlane ids and file ids that are closed. Everything else is open. */
  collapsed: ReadonlySet<string>;
  onCollapsedChange: (next: Set<string>) => void;
};

export function TextResults({ groups, lanes, collapsed, onCollapsedChange }: Props) {
  const goTo = useNavigateTo();
  const nodes = useMemo(() => buildNodes(groups, lanes), [groups, lanes]);
  const hitAt = useMemo(() => {
    const map = new Map<string, string>();
    for (const group of groups) for (const file of group.files) for (const { id, hit } of file.hits) map.set(id, linkTo("files", { id: group.swimlane }, { path: hit.path, line: String(hit.line) }));
    return map;
  }, [groups]);
  const expanded = useMemo(() => {
    const open = new Set<string>();
    for (const group of groups) {
      if (!collapsed.has(group.id)) open.add(group.id);
      for (const file of group.files) if (!collapsed.has(file.id)) open.add(file.id);
    }
    return open;
  }, [groups, collapsed]);

  const onExpandedChange = useCallback(
    (next: Set<string>, changed: string) => {
      const closed = new Set(collapsed);
      if (next.has(changed)) closed.delete(changed);
      else closed.add(changed);
      onCollapsedChange(closed);
    },
    [collapsed, onCollapsedChange],
  );

  const onSelect = useCallback(
    (id: string) => {
      const href = hitAt.get(id);
      if (href !== undefined) {
        goTo(href);
        return;
      }
      // A swimlane or a file: choosing it opens or closes it.
      const closed = new Set(collapsed);
      if (closed.has(id)) closed.delete(id);
      else closed.add(id);
      onCollapsedChange(closed);
    },
    [hitAt, goTo, collapsed, onCollapsedChange],
  );

  // Control or Command and Enter on a hit opens the file at that line in a new tab; a swimlane or a file row has no address of its own.
  const onOpenInNewTab = useCallback(
    (id: string) => {
      const href = hitAt.get(id);
      if (href !== undefined) openInNewTab(href);
    },
    [hitAt],
  );

  return <TreeView label="Results by swimlane and file" nodes={nodes} expanded={expanded} onExpandedChange={onExpandedChange} onSelect={onSelect} onOpenInNewTab={onOpenInNewTab} className="min-h-0 flex-1" />;
}
