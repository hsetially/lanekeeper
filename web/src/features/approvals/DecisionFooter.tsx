import { useId } from "react";
import type { Proposal } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { visibleText } from "@/components/app/safe-text";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import type { GateResult } from "@/components/app/write-gate";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { CircleX, Check, History } from "@/icons";
import { AppLink } from "@/routes/AppLink";
import type { Decide } from "./model";

// The foot of the drawer: the reason field, Reject and Approve. A button that can not be pressed is disabled with the reason, not
// hidden (the server still decides: it answers 403 for your own proposal and 422 for one that is no longer pending). Reject needs
// a reason of at least three characters, as the design says; the contract takes at most 256. Neither button does anything while
// a decision is on its way, and neither is ever optimistic: the drawer shows what the server answers.

export const MIN_REASON = 3;
export const MAX_REASON = 256;

type Props = {
  p: Proposal;
  authorName: string;
  decide: Decide;
  /** The permission and the connection, from the write gate: they come before anything about the proposal. */
  base: GateResult;
  reason: string;
  onReason: (reason: string) => void;
  working: "approve" | "reject" | null;
  onApprove: () => void;
  onReject: () => void;
  error: WriteErrorInfo | null;
};

function pick(base: GateResult, own: GateResult, extra: GateResult): GateResult {
  if (!base.allowed) return base;
  if (!own.allowed) return own;
  return extra;
}

export function DecisionFooter({ p, authorName, decide, base, reason, onReason, working, onApprove, onReject, error }: Props) {
  const field = useId();
  const closed = decide.note === "expired" || decide.note === "decided";
  if (closed) {
    return (
      <div className="flex flex-none flex-col gap-2 border-t border-border px-4 pt-2.5 pb-3">
        {error !== null ? <WriteError {...error} /> : null}
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-muted-foreground">{decide.note === "expired" ? "Expired proposals can't be decided." : "Decision recorded in the audit log."}</span>
          <Button asChild variant="ghost" size="sm" className="ml-auto">
            <AppLink route="audit" search={{ swimlane: p.swimlane }}>
              <History data-icon="inline-start" />
              View in audit log
            </AppLink>
          </Button>
        </div>
      </div>
    );
  }

  const busy: GateResult = { allowed: false, reason: "A decision is being recorded" };
  const needReason: GateResult = reason.trim().length < MIN_REASON ? { allowed: false, reason: "Write a reason to reject" } : { allowed: true, reason: null };
  const approveGate = working === "reject" ? busy : pick(base, decide.approve, { allowed: true, reason: null });
  const rejectGate = working === "approve" ? busy : pick(base, decide.reject, needReason);
  const own = decide.note === "own";

  return (
    <div role="group" aria-label="Decision" className="flex flex-none flex-col gap-2 border-t border-border px-4 pt-2.5 pb-3">
      {error !== null ? <WriteError {...error} /> : null}
      {own ? null : (
        <div className="flex flex-col gap-1">
          <Label htmlFor={field} className="text-xs font-normal text-muted-foreground">
            Reason (required to reject)
          </Label>
          <Input
            id={field}
            value={reason}
            maxLength={MAX_REASON}
            autoComplete="off"
            disabled={working !== null}
            placeholder={`Tell ${visibleText(authorName)} what to change`}
            onChange={(event) => {
              onReason(event.target.value);
            }}
          />
        </div>
      )}
      <div className="flex items-center justify-between gap-2">
        <GatedButton variant="secondary" gate={rejectGate} loading={working === "reject"} className="border-danger-border text-danger-fg" onClick={onReject}>
          <CircleX data-icon="inline-start" />
          {working === "reject" ? "Rejecting…" : "Reject"}
        </GatedButton>
        <GatedButton gate={approveGate} loading={working === "approve"} onClick={onApprove}>
          <Check data-icon="inline-start" />
          {working === "approve" ? "Approving…" : "Approve"}
        </GatedButton>
      </div>
    </div>
  );
}
