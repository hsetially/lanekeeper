import { useMemo } from "react";
import type { AuditEvent } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { Chip } from "@/features/approvals/chips";
import type { People } from "@/features/approvals/people";
import { CircleHelp, ShieldAlert } from "@/icons";
import { formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { actionView, confidenceView, sourceLabel, viaView, whoOf, type Break } from "./model";

// The log: one row per event, 48 px tall (a row says two things in four of its cells), newest first, in the design's columns but
// the two the contract has nothing for (the hashes of the file before and after, and the approval). It is the shared DataTable (rows
// virtualised, the cursor is `aria-activedescendant`, the keys of the keymap). Every piece of text from a person or a file goes
// through SafeText.

export const ROW_HEIGHT = 48;

/** `2025-10-09 14:44:02 UTC` as the two lines the column draws: the clock, then the day and the sequence number. */
export function whenParts(at: number): { clock: string; day: string } {
  const text = formatTime(at, { seconds: true });
  return text === "unknown time" ? { clock: text, day: "" } : { clock: text.slice(11, 19), day: text.slice(0, 10) };
}

function Who({ event, people }: { event: AuditEvent; people: People }) {
  const who = whoOf(event, people.name);
  if (who.kind === "unknown") {
    return (
      <Chip tone="unknown" Icon={CircleHelp}>
        unknown author
      </Chip>
    );
  }
  if (who.kind === "system") return <span className="text-muted-foreground">Lanekeeper</span>;
  return (
    <>
      <b className="max-w-full truncate font-medium">
        <SafeText text={who.name} />
      </b>
      {who.probable ? <small className="text-xs text-muted-foreground">probable</small> : null}
    </>
  );
}

function Attribution({ event }: { event: AuditEvent }) {
  const a = event.attribution;
  if (a === null) return <span className="text-muted-foreground">–</span>;
  const c = confidenceView(a.confidence);
  return (
    <>
      <span className={cn("max-w-full truncate text-xs", a.source === "unknown" && "text-muted-foreground")}>{sourceLabel(a.source)}</span>
      <span className="inline-flex items-center gap-1.5 font-mono text-xs text-muted-foreground">
        <span aria-hidden="true" className="block h-1 w-9 overflow-hidden rounded-[2px] bg-accent">
          <span className={cn("block h-full", c.low ? "bg-warning-fg" : "bg-foreground")} style={{ width: `${c.bar}%` }} />
        </span>
        {c.label}
      </span>
    </>
  );
}

function WhenCell({ event, broken }: { event: AuditEvent; broken: boolean }) {
  const when = whenParts(event.at);
  return (
    <span className="flex flex-col font-mono text-xs leading-4 tabular-nums" title={formatTime(event.at, { seconds: true })}>
      <b className="font-medium">{when.clock}</b>
      <span className="truncate text-muted-foreground">
        {when.day} · #{event.seq.toLocaleString("en-US")}
      </span>
      {broken ? (
        <Chip tone="rejected" Icon={ShieldAlert} className="h-4 border-danger-fg bg-danger-fg text-background">
          Chain break
        </Chip>
      ) : null}
    </span>
  );
}

function TargetCell({ event }: { event: AuditEvent }) {
  return (
    <span className="flex min-w-0 flex-col">
      {event.target === null ? (
        <span className="text-muted-foreground">–</span>
      ) : (
        <b className="truncate font-mono text-xs font-medium">
          <SafeText text={event.target} />
        </b>
      )}
      {event.request_id === null ? null : (
        <span className="truncate font-mono text-xs text-muted-foreground">
          <SafeText text={event.request_id} />
        </span>
      )}
    </span>
  );
}

type Props = {
  rows: readonly AuditEvent[];
  selectedId: number | null;
  breaks: ReadonlyMap<number, Break>;
  people: People;
  onOpen: (event: AuditEvent) => void;
  /** The hub has more events than are loaded: the table does not know how many there are in all. */
  moreRows?: boolean;
  className?: string;
};

const keyOf = (event: AuditEvent): string => String(event.id);

export function AuditTable({ rows, selectedId, breaks, people, onOpen, moreRows = false, className }: Props) {
  const columns = useMemo<Column<AuditEvent>[]>(
    () => [
      { id: "time", header: "Time", width: "148px", minWidth: 148, cell: (event) => <WhenCell event={event} broken={breaks.has(event.id)} /> },
      {
        id: "user",
        header: "User",
        width: "156px",
        minWidth: 156,
        cell: (event) => (
          <span className="flex min-w-0 flex-col items-start gap-0.5">
            <Who event={event} people={people} />
          </span>
        ),
      },
      {
        id: "via",
        header: "Via",
        width: "92px",
        minWidth: 92,
        cell: (event) => {
          const via = viaView(event);
          return (
            <span title={via.hint}>
              <Chip tone={via.tone} Icon={via.Icon}>
                {via.label}
              </Chip>
            </span>
          );
        },
      },
      {
        id: "action",
        header: "Action",
        width: "156px",
        minWidth: 156,
        cell: (event) => {
          const action = actionView(event.action);
          return (
            <span className="flex min-w-0 items-center gap-1.5">
              <action.Icon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="truncate">
                <SafeText text={action.label} />
              </span>
            </span>
          );
        },
      },
      {
        id: "swimlane",
        header: "Swimlane",
        width: "80px",
        minWidth: 80,
        cell: (event) => <span className="font-mono text-xs">{event.swimlane === null ? <span className="text-muted-foreground">–</span> : <SafeText text={event.swimlane} />}</span>,
      },
      { id: "target", header: "Target", width: "minmax(0,1fr)", minWidth: 232, cell: (event) => <TargetCell event={event} /> },
      {
        id: "attribution",
        header: "Attribution",
        width: "148px",
        minWidth: 148,
        cell: (event) => (
          <span className="flex min-w-0 flex-col gap-0.5">
            <Attribution event={event} />
          </span>
        ),
      },
    ],
    [breaks, people],
  );
  const selectedKeys = useMemo<ReadonlySet<string>>(() => new Set(selectedId === null ? [] : [String(selectedId)]), [selectedId]);
  return (
    <DataTable
      label="Audit events"
      rows={rows}
      columns={columns}
      rowKey={keyOf}
      rowHeight={ROW_HEIGHT}
      selectedKeys={selectedKeys}
      // A link in the chain that does not follow from the one before it: the row is marked, unless it is the open one.
      rowClassName={(event) => (breaks.has(event.id) && event.id !== selectedId ? "bg-danger-bg shadow-[inset_3px_0_0_var(--danger-fg)]" : undefined)}
      onActivate={onOpen}
      onRowClick={onOpen}
      moreRows={moreRows}
      {...(className === undefined ? {} : { className })}
    />
  );
}
