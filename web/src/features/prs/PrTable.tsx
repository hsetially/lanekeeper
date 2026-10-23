import { useMemo, useRef } from "react";
import type { PrLink } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { userLabel } from "@/components/attribution/model";
import { prAddress } from "@/components/pr/pr-url";
import { PrStateChip } from "@/components/pr/PrStateChip";
import { useActorNames } from "@/features/attribution/names";
import { formatRelative, formatTime } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";

// The pull requests of a swimlane, one row for each link (a pull request that carries three files has three links, and so three
// rows). A row is 32 px so the list can be virtualised. The PR number is the row's own link, to GitHub in another tab, and Enter on
// the row follows it; the file is a link into the app. The table never sorts or filters: the server's order (newest first) is the order.
//
// A dense row has no copy button (copy inventory, rows 10, 13, 21); the dialog that raised the pull request has the address's copy.

export const ROW_HEIGHT = 32;

const keyOf = (link: PrLink): string => String(link.id);

type Props = {
  rows: readonly PrLink[];
  swimlane: string;
  /** The signed-in person, to say "You". */
  me: string | null;
  now: number;
  /** The server has more links than are loaded. */
  moreRows: boolean;
  className?: string;
};

function PrNumber({ link }: { link: PrLink }) {
  const address = prAddress(link.pr_url);
  if (address === null) return <span className="font-mono text-xs">#{link.pr_number}</span>;
  return (
    <a data-pr-link={link.id} href={address.href} target="_blank" rel="noopener noreferrer" className="font-mono text-xs">
      #{link.pr_number}
      <span className="sr-only">, on GitHub</span>
    </a>
  );
}

export function PrTable({ rows, swimlane, me, now, moreRows, className }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const { nameOf } = useActorNames();
  const columns = useMemo<Column<PrLink>[]>(
    () => [
      { id: "state", header: "State", width: "96px", minWidth: 96, cell: (link) => <PrStateChip state={link.state} /> },
      { id: "number", header: "Pull request", width: "112px", minWidth: 112, cell: (link) => <PrNumber link={link} /> },
      {
        id: "file",
        header: "File",
        width: "minmax(0,1fr)",
        minWidth: 240,
        cell: (link) => (
          <AppLink route="files" params={{ id: swimlane }} search={{ path: link.path }} className="font-mono text-xs text-foreground no-underline hover:underline">
            <SafeText text={link.path} />
          </AppLink>
        ),
      },
      {
        id: "by",
        header: "Opened by",
        width: "160px",
        minWidth: 160,
        cell: (link) => (
          <span className="min-w-0 truncate text-xs">
            <SafeText text={userLabel(link.author, { you: me, nameOf })} />
          </span>
        ),
      },
      {
        id: "opened",
        header: "Opened",
        width: "104px",
        minWidth: 104,
        cell: (link) => (
          <span className="text-xs tabular-nums" title={formatTime(link.created_at)}>
            {formatRelative(link.created_at, now)}
          </span>
        ),
      },
      {
        id: "ended",
        header: "Merged or closed",
        width: "136px",
        minWidth: 136,
        cell: (link) =>
          link.closed_at === null ? (
            <span className="text-xs text-muted-foreground">—</span>
          ) : (
            <span className="text-xs tabular-nums" title={formatTime(link.closed_at)}>
              {formatRelative(link.closed_at, now)}
            </span>
          ),
      },
    ],
    [swimlane, me, nameOf, now],
  );
  return (
    <div ref={box} className={className}>
      <DataTable
        label="Pull requests"
        rows={rows}
        columns={columns}
        rowKey={keyOf}
        rowHeight={ROW_HEIGHT}
        // Enter follows the row's own link: the pull request, on GitHub. The link is in the row, checked as an address, so a row
        // whose address is not https has nothing to follow.
        onActivate={(link) => box.current?.querySelector<HTMLAnchorElement>(`a[data-pr-link="${link.id}"]`)?.click()}
        moreRows={moreRows}
        className="min-h-0 flex-1"
      />
    </div>
  );
}
