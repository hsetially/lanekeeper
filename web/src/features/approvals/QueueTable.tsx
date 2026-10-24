import { useMemo } from "react";
import type { Proposal } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { TriangleAlert } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { openInNewTab } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import { Chip } from "./chips";
import { actionView, decisionLine, expiresIn, fileParts, statusView, type Tab } from "./model";
import { firstName, type People } from "./people";

// The queue (Approvals page): one row per proposal, 48 px tall because a row says two things in two of its cells (who and how,
// the file and where it is), in the design's six columns. It is the shared DataTable (rows virtualised, the cursor is
// `aria-activedescendant`, the keys of the keymap): Enter or a click opens the proposal in the drawer, Control or Command and Enter
// opens it in a new tab. The table never sorts or filters: the server's order (newest first) is the order.

export const ROW_HEIGHT = 48;

/** The last column's title depends on the tab: when it runs out, when it ran out, or what was decided. */
export function lastColumn(tab: Tab): string {
  return tab === "pending" || tab === "stale" ? "Expires in" : tab === "expired" ? "Expired" : "Decision";
}

/** "12 min", "3 h", "just now": the age without the word "ago". */
const age = (ms: number, now: number): string => formatRelative(ms, now).replace(/ ago$/, "");

type Props = {
  tab: Tab;
  rows: readonly Proposal[];
  selectedId: number | null;
  /** Proposals that arrived while the page was open and have been shown: tinted, with a New chip. */
  fresh: ReadonlySet<number>;
  me: string | null;
  people: People;
  now: number;
  onOpen: (proposal: Proposal) => void;
  /** The hub has more proposals than are loaded: the table does not know how many there are in all. */
  moreRows?: boolean;
  /** The keyboard cursor starts on this proposal's row (the queue was drawn anew after its drawer closed). */
  cursorOn?: number | null;
  className?: string;
};

function Last({ p, tab, now, people }: { p: Proposal; tab: Tab; now: number; people: People }) {
  if (tab === "decided") {
    const d = decisionLine(p);
    if (d === null) return <span className="text-xs text-muted-foreground">{statusView(p.status).label}</span>;
    return (
      <span className="truncate text-xs" title={d.at === null ? undefined : formatTime(d.at)}>
        <SafeText text={d.by === null ? d.verb : `${d.verb} · ${firstName(people.name(d.by))}`} />
      </span>
    );
  }
  const e = expiresIn(p, now);
  return (
    <span className={cn("text-xs tabular-nums", e.urgent && "font-semibold text-warning-fg")} title={formatTime(p.expires_at)}>
      {e.text}
    </span>
  );
}

/** The author, and under it what else is true of the proposal: how it arrived, that it is yours, that it is new. */
function AuthorCell({ p, me, people, isNew }: { p: Proposal; me: string | null; people: People; isNew: boolean }) {
  return (
    <span className="flex min-w-0 flex-col items-start gap-0.5">
      <b className="max-w-full truncate font-medium">
        <SafeText text={people.name(p.author)} />
      </b>
      <span className="flex flex-wrap items-center gap-1">
        {p.via === "mcp" ? <Chip tone="cursor">via Cursor</Chip> : null}
        {me !== null && p.author === me ? <span className="inline-flex h-[18px] items-center rounded-sm border border-input px-1.5 text-xs font-medium">You</span> : null}
        {isNew ? <Chip tone="pr">New</Chip> : null}
      </span>
    </span>
  );
}

/** The file's name, and under it where it lives, or that it changed since the proposal was made. */
function FileCell({ p }: { p: Proposal }) {
  const { name, where } = fileParts(p);
  return (
    <span className="flex min-w-0 flex-col items-start gap-0.5">
      <b className="max-w-full truncate font-mono text-xs font-medium">
        <SafeText text={name} />
      </b>
      {p.status === "stale" ? (
        <Chip tone="stale" Icon={TriangleAlert}>
          File changed since proposed
        </Chip>
      ) : (
        <span className="max-w-full truncate font-mono text-xs text-muted-foreground">
          <SafeText text={where} />
        </span>
      )}
    </span>
  );
}

const keyOf = (p: Proposal): string => String(p.id);

export function QueueTable({ tab, rows, selectedId, fresh, me, people, now, onOpen, moreRows = false, cursorOn = null, className }: Props) {
  const columns = useMemo<Column<Proposal>[]>(
    () => [
      { id: "author", header: "Author", width: "176px", minWidth: 176, cell: (p) => <AuthorCell p={p} me={me} people={people} isNew={fresh.has(p.id)} /> },
      {
        id: "action",
        header: "Action",
        width: "148px",
        minWidth: 148,
        cell: (p) => {
          const action = actionView(p);
          return (
            <Chip tone={action.tone} Icon={action.Icon}>
              {action.label}
            </Chip>
          );
        },
      },
      {
        id: "swimlane",
        header: "Swimlane",
        width: "64px",
        minWidth: 64,
        cell: (p) => (
          <span className="font-mono text-xs">
            <SafeText text={p.swimlane} />
          </span>
        ),
      },
      { id: "file", header: "File", width: "minmax(0,1fr)", minWidth: 252, cell: (p) => <FileCell p={p} /> },
      {
        id: "age",
        header: "Age",
        width: "72px",
        minWidth: 72,
        cell: (p) => (
          <span className="text-xs tabular-nums" title={formatTime(p.created_at)}>
            {age(p.created_at, now)}
          </span>
        ),
      },
      { id: "last", header: lastColumn(tab), width: "128px", minWidth: 128, cell: (p) => <Last p={p} tab={tab} now={now} people={people} /> },
    ],
    [tab, me, people, now, fresh],
  );
  const selectedKeys = useMemo<ReadonlySet<string>>(() => new Set(selectedId === null ? [] : [String(selectedId)]), [selectedId]);

  return (
    <DataTable
      label="Proposals"
      rows={rows}
      columns={columns}
      rowKey={keyOf}
      rowHeight={ROW_HEIGHT}
      selectedKeys={selectedKeys}
      // A proposal that arrived while the page was open and has been shown: tinted, with a New chip. The open one keeps the selection colour.
      rowClassName={(p) => (fresh.has(p.id) && p.id !== selectedId ? "bg-info-bg shadow-[inset_2px_0_0_var(--info-fg)]" : undefined)}
      onActivate={onOpen}
      onRowClick={onOpen}
      onOpenInNewTab={(p) => {
        openInNewTab(linkTo("approval", { id: String(p.id) }));
      }}
      moreRows={moreRows}
      {...(cursorOn === null ? {} : { initialActiveKey: String(cursorOn) })}
      {...(className === undefined ? {} : { className })}
    />
  );
}
