import type { ReactNode } from "react";
import type { BaselineState } from "@/api/generated/model";
import { HashChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { ProblemDialog } from "@/components/app/ProblemDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";

// The three answers to a write that is not a plain success, in the words of the States index (C01, K02, A01). Each says first
// that NOTHING was applied (or, for the last, that it has not been applied yet), then who or what is in the way, then the way
// forward. A dialog, not a toast: it is the person's own write, and it must stay until they have read it (H-3).

function Sub({ code, children }: { code?: string; children: ReactNode }) {
  return (
    <p className="flex flex-wrap items-center gap-2 font-mono text-xs text-muted-foreground">
      {code ? <Badge className="font-mono">{code}</Badge> : null}
      {children}
    </p>
  );
}

function Facts({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">{children}</dl>;
}
const Term = ({ children }: { children: ReactNode }) => <dt className="text-muted-foreground">{children}</dt>;
const Def = ({ children }: { children: ReactNode }) => <dd className="m-0 flex min-w-0 flex-wrap items-center gap-1.5">{children}</dd>;

type Base = { open: boolean; onOpenChange: (open: boolean) => void };

// ---- 409

type ConflictProps = Base & {
  /** The file's name, as it reads in the sub line. */
  file: string;
  lane: string;
  /** The hash the person's edit was based on. */
  yourBase: string | null;
  /** The hash on NFS now, or null when the file was removed. */
  current: string | null;
  /** What the version history says about that version ("Tool write, 2 min ago"), when it is known. */
  currentNote: string | null;
  requestId?: string | undefined;
  /**
   * What the write was when it was not an edit that is still open ("revert", "upload", "delete"). The person has no text of their
   * own to keep then, so the dialog has only Close, and the three handlers below are not used.
   */
  what?: string | undefined;
  /** Throw the edit away and show the file as it is on NFS now. */
  onDiscard?: () => void;
  /** Show what is on NFS now beside the person's version. */
  onViewCurrent?: () => void;
  /** Keep the person's version, and base the next save on what is on NFS now. */
  onReapply?: () => void;
};

export function ConflictDialog({ open, onOpenChange, file, lane, yourBase, current, currentNote, requestId, what, onDiscard, onViewCurrent, onReapply }: ConflictProps) {
  const removed = current === null;
  const other = what !== undefined;
  return (
    <ProblemDialog
      open={open}
      onOpenChange={onOpenChange}
      variant="conflict"
      title={other ? "The file changed on NFS first" : "File changed on NFS while you were editing"}
      description={
        other
          ? removed
            ? `Your ${what} was not applied. The file was removed from NFS first.`
            : `Your ${what} was not applied. Nothing on NFS changed. Reload the file and try again.`
          : removed
            ? "Your save was not applied. The file was removed from NFS while you were editing."
            : "Your save was not applied. Nothing on NFS changed."
      }
      {...(requestId === undefined ? {} : { requestId })}
      actions={
        other ? (
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        ) : (
          <>
            <Button variant="ghost" onClick={onDiscard}>
              Discard my changes
            </Button>
            {removed ? null : (
              <Button variant="secondary" onClick={onViewCurrent}>
                View current
              </Button>
            )}
            <Button onClick={onReapply}>{removed ? "Save as a new file" : "Reload and re-apply"}</Button>
          </>
        )
      }
    >
      <Sub code="409">
        <SafeText text={`${file} · ${lane}`} />
      </Sub>
      <Facts>
        <Term>{other ? "Based on" : "Your base"}</Term>
        <Def>{yourBase === null ? <span>None: the file was new</span> : <HashChip hash={yourBase} />}</Def>
        <Term>On NFS now</Term>
        <Def>
          {current === null ? (
            <span>The file is removed</span>
          ) : (
            <>
              <HashChip hash={current} />
              {currentNote === null ? null : <SafeText text={currentNote} />}
            </>
          )}
        </Def>
      </Facts>
    </ProblemDialog>
  );
}

// ---- 423

type LockedProps = Base & {
  /** What was attempted, as the sub line reads: "Save to presitc". */
  action: string;
  lane: string;
  /** The swimlane's baseline, which is what locks editing today (the lock carries no cause of its own). */
  baseline: BaselineState | undefined;
  /** How many files are unknown, when the swimlane says. */
  unknownFiles: number | null;
  /** When the lock ends, when the server says. */
  until: number | null;
  /** The person may start the adoption (Admin). */
  canAdopt: boolean;
  requestId?: string | undefined;
};

export function LockedDialog({ open, onOpenChange, action, lane, baseline, unknownFiles, until, canAdopt, requestId }: LockedProps) {
  const unconfirmed = baseline === "unconfirmed";
  const adopting = baseline === "in_progress";
  const title = unconfirmed ? `${lane} is locked until it is adopted` : adopting ? `${lane} is being adopted` : "Editing is locked";
  const lead = unconfirmed
    ? `Its baseline is unconfirmed, so edits are blocked. An admin needs to adopt ${unknownFiles !== null && unknownFiles > 0 ? `${unknownFiles.toLocaleString("en-US")} unknown files` : "the unknown files"} first.`
    : adopting
      ? "An adoption is open, so edits are blocked until an Admin confirms it."
      : "The target is locked by another operation, so nothing was applied. Try again in a moment.";
  return (
    <ProblemDialog
      open={open}
      onOpenChange={onOpenChange}
      variant="locked"
      title={title}
      description={lead}
      {...(requestId === undefined ? {} : { requestId })}
      actions={
        <>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Close
          </Button>
          {(unconfirmed || adopting) && canAdopt ? (
            <Button asChild>
              <AppLink route="adopt" params={{ id: lane }}>
                {adopting ? "Open the adoption" : "Start adoption"}
              </AppLink>
            </Button>
          ) : null}
        </>
      }
    >
      <Sub code="423">
        <SafeText text={action} />
      </Sub>
      {unconfirmed || adopting || until !== null ? (
        <Facts>
          {unconfirmed || adopting ? (
            <>
              <Term>Baseline</Term>
              <Def>{adopting ? "Adoption in progress" : "Unconfirmed"}</Def>
            </>
          ) : null}
          {unconfirmed && unknownFiles !== null ? (
            <>
              <Term>Unknown files</Term>
              <Def>{unknownFiles.toLocaleString("en-US")}</Def>
            </>
          ) : null}
          {until === null ? null : (
            <>
              <Term>Lock</Term>
              <Def>Locked until {formatTime(until)}</Def>
            </>
          )}
        </Facts>
      ) : null}
      {(unconfirmed || adopting) && !canAdopt ? <p className="text-sm text-muted-foreground">Ask an Admin to start the adoption.</p> : null}
    </ProblemDialog>
  );
}

// ---- sent for approval

type SentProps = Base & {
  file: string;
  lane: string;
  proposal: number;
  /** Approvals is open to this person (Admin only today, W12). */
  canOpenApprovals: boolean;
};

export function SentDialog({ open, onOpenChange, file, lane, proposal, canOpenApprovals }: SentProps) {
  return (
    <ProblemDialog
      open={open}
      onOpenChange={onOpenChange}
      variant="approval"
      title="Sent for approval"
      description="Your change needs an Admin to approve it. Nothing has changed on NFS yet."
      actions={
        <>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Stay on file
          </Button>
          {canOpenApprovals ? (
            <Button asChild>
              <AppLink route="approval" params={{ id: String(proposal) }}>
                View request
              </AppLink>
            </Button>
          ) : null}
        </>
      }
    >
      <Sub>
        <SafeText text={`${file} · ${lane}`} />
      </Sub>
      <Facts>
        <Term>Request</Term>
        <Def>
          <span className="font-mono">AP-{proposal}</span>
          <StatusBadge variant="approval" />
        </Def>
        <Term>Waiting on</Term>
        <Def>Any Admin</Def>
      </Facts>
    </ProblemDialog>
  );
}
