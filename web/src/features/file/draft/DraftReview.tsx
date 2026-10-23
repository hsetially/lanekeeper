import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { Draft, ProposalStatus } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { Banner } from "@/components/app/Banner";
import { DiffView } from "@/components/app/diff/DiffView";
import { GatedButton } from "@/components/app/GatedButton";
import { HashChip, PathChip } from "@/components/app/chips";
import { ProblemDialog } from "@/components/app/ProblemDialog";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { useWriteGate } from "@/components/app/write-gate";
import { SeverityBadge } from "@/components/attribution";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { Columns2, Rows2 } from "@/icons";
import { formatTime } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";
import { SettingChips } from "../diff/ChangeSummary";
import { summarise } from "../diff/summary";
import type { WriteOutcome } from "../edit/outcome";
import { LockedDialog, SentDialog } from "../edit/OutcomeDialogs";
import { splitPath, unknownFileCount } from "../model";
import { ApplyDraftDialog } from "./ApplyDraftDialog";
import { cannotApply, closedBy, MAX_EFFECTS, pickupVariant } from "./model";

// A draft, ready to read before it is applied (Draft, review). Everything here is the server's: the diff and the settings that
// change are the engine's, the severity comes from the severity rules, the services and when each would pick the change up are
// worked out as of the moment of the read. Applying is the one write on the page, and its answer is shown, never assumed: it is
// applied, sent for approval, refused because the file changed since (409), or locked (423).

type Settled = { kind: "applied" } | { kind: "sent"; proposal: number } | { kind: "conflict"; requestId: string | undefined } | { kind: "locked"; until: number | null; requestId: string | undefined };

const STATUS_LABEL: Record<ProposalStatus, string> = { draft: "Draft", pending: "Sent for approval", approved: "Approved", rejected: "Rejected", stale: "Out of date", expired: "Expired", applied: "Applied" };

function Fact({ term, children }: { term: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{term}</dt>
      <dd className="m-0 flex min-w-0 flex-wrap items-center gap-1.5">{children}</dd>
    </>
  );
}

export type DraftReviewProps = {
  draft: Draft;
  /** Read the draft again (its status changes when it is applied). */
  onRefresh: () => void;
};

export function DraftReview({ draft, onRefresh }: DraftReviewProps) {
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  const base = useWriteGate("editor");
  const [applying, setApplying] = useState(false);
  const [settled, setSettled] = useState<Settled | null>(null);
  const [layout, setLayout] = useState<"split" | "unified">("split");

  const locked = settled?.kind === "locked";
  const detail = useQuery({ ...swimlaneQuery(draft.swimlane), enabled: locked }).data;

  const { name } = splitPath(draft.path);
  const closed = closedBy(draft.status);
  const reason = cannotApply(draft.status);
  const gate = !base.allowed ? base : reason !== null ? { allowed: false, reason } : base;
  const summary = summarise(draft.setting_changes);
  const effects = draft.effects ?? [];
  const justApplied = settled?.kind === "applied";

  const onSettled = (outcome: WriteOutcome): void => {
    if (outcome.kind === "applied") setSettled({ kind: "applied" });
    else if (outcome.kind === "sent") setSettled({ kind: "sent", proposal: outcome.proposal });
    else if (outcome.kind === "conflict") setSettled({ kind: "conflict", requestId: outcome.requestId });
    else if (outcome.kind === "locked") setSettled({ kind: "locked", until: outcome.until, requestId: outcome.requestId });
    // Whatever it was, the draft may have a new status now.
    onRefresh();
  };

  const closeSettled = (open: boolean): void => {
    if (!open) setSettled(null);
  };

  const openFile = (
    <Button asChild variant="secondary">
      <AppLink route="files" params={{ id: draft.swimlane }} search={{ path: draft.path }}>
        Open the file
      </AppLink>
    </Button>
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      {justApplied ? (
        <Banner variant="success" title="Draft applied" onDismiss={() => setSettled(null)}>
          The change is on {draft.swimlane} NFS.
        </Banner>
      ) : closed === null ? null : (
        <Banner variant={closed.tone} title={closed.title}>
          {closed.text}
        </Banner>
      )}
      <section aria-label="Draft" className="flex flex-none flex-col gap-3 px-6 py-4">
        <dl className="m-0 grid grid-cols-[96px_minmax(0,1fr)] items-center gap-x-3 gap-y-2 text-sm">
          <Fact term="Swimlane">
            <AppLink route="swimlane" params={{ id: draft.swimlane }}>
              <SafeText text={draft.swimlane} />
            </AppLink>
          </Fact>
          <Fact term="File">
            <PathChip path={draft.path} />
          </Fact>
          <Fact term="Status">{STATUS_LABEL[draft.status]}</Fact>
          <Fact term="Based on">{draft.expected.state === "hash" ? <HashChip hash={draft.expected.hash} /> : <span>No file: applying this creates it</span>}</Fact>
          <Fact term="New version">
            <HashChip hash={draft.new_hash} />
          </Fact>
          <Fact term="Severity">{draft.severity === undefined || draft.severity === null ? <span className="text-muted-foreground">Not rated</span> : <SeverityBadge severity={draft.severity} />}</Fact>
          <Fact term="Kept until">{formatTime(draft.expires_at)}</Fact>
        </dl>
        {effects.length === 0 ? null : (
          <div className="flex flex-col gap-1.5">
            <strong className="font-semibold">When services pick this up</strong>
            <ul aria-label="Services" className="m-0 flex list-none flex-col gap-1 p-0">
              {effects.slice(0, MAX_EFFECTS).map(([service, state], i) => (
                <li key={`${service.namespace}/${service.name}:${i}`} className="flex flex-wrap items-center gap-2">
                  <span className="font-mono text-xs">
                    <SafeText text={`${service.namespace}/${service.name}`} />
                  </span>
                  <StatusBadge variant={pickupVariant(state)} />
                </li>
              ))}
              {effects.length > MAX_EFFECTS ? <li className="text-xs text-muted-foreground">and {effects.length - MAX_EFFECTS} more services.</li> : null}
            </ul>
          </div>
        )}
      </section>
      <SettingChips summary={summary} />
      <div className="flex flex-none items-center justify-between border-b border-border bg-muted px-6 py-1.5">
        <strong className="font-semibold">Changes</strong>
        <ToggleGroup type="single" value={layout} onValueChange={(value) => (value === "split" || value === "unified") && setLayout(value)} aria-label="Diff layout">
          <ToggleGroupItem value="split" aria-label="Side by side">
            <Columns2 aria-hidden="true" />
            Side by side
          </ToggleGroupItem>
          <ToggleGroupItem value="unified" aria-label="Unified">
            <Rows2 aria-hidden="true" />
            Unified
          </ToggleGroupItem>
        </ToggleGroup>
      </div>
      <div className="flex h-[28rem] flex-none flex-col">
        <DiffView label={`Changes to ${name}`} hunks={draft.hunks} mode={layout} className="min-h-0 flex-1" />
      </div>
      <div className="sticky bottom-0 flex flex-none items-center justify-end gap-2 border-t border-border bg-background px-6 py-3">
        {openFile}
        <GatedButton gate={gate} onClick={() => setApplying(true)}>
          Apply draft
        </GatedButton>
      </div>

      <ApplyDraftDialog open={applying} onOpenChange={setApplying} draft={draft} onSettled={onSettled} />
      <SentDialog open={settled?.kind === "sent"} onOpenChange={closeSettled} file={name} lane={draft.swimlane} proposal={settled?.kind === "sent" ? settled.proposal : 0} canOpenApprovals={roleAtLeast(role, "admin")} />
      <ProblemDialog
        open={settled?.kind === "conflict"}
        onOpenChange={closeSettled}
        variant="conflict"
        title="The file changed since this draft was made"
        description="Nothing was applied. A draft is applied only when the file is still as it was. Open the file and make the change again."
        {...(settled?.kind === "conflict" && settled.requestId !== undefined ? { requestId: settled.requestId } : {})}
        actions={
          <>
            <Button variant="secondary" onClick={() => setSettled(null)}>
              Close
            </Button>
            {openFile}
          </>
        }
      />
      <LockedDialog
        open={locked}
        onOpenChange={closeSettled}
        action={`Apply draft to ${draft.swimlane}`}
        lane={draft.swimlane}
        baseline={detail?.baseline?.state}
        unknownFiles={unknownFileCount(detail?.drift_counts)}
        until={settled?.kind === "locked" ? settled.until : null}
        canAdopt={roleAtLeast(role, "admin")}
        {...(settled?.kind === "locked" && settled.requestId !== undefined ? { requestId: settled.requestId } : {})}
      />
    </div>
  );
}
