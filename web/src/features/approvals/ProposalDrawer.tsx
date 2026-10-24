import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Proposal } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { ProblemDialog } from "@/components/app/ProblemDialog";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { HashChip } from "@/components/app/chips";
import { CopyButton } from "@/components/app/CopyButton";
import { CopyLink } from "@/components/app/CopyLink";
import { COPY_GROUP, COPY_REVEAL } from "@/components/app/CopyValue";
import { useWriteGate } from "@/components/app/write-gate";
import { Button } from "@/components/ui/button";
import { X } from "@/icons";
import { useFocusOnOpen } from "@/features/shell/focus";
import { useQueryState } from "@/lib/query/use-query-state";
import { formatTime } from "@/lib/time";
import { linkTo } from "@/routes/links";
import { useNow } from "@/live/use-now";
import { bannerOf, type Answer } from "./banner";
import { Chip } from "./chips";
import { DecisionFooter } from "./DecisionFooter";
import { dialogOf, failureOf } from "./failure";
import { approvalMutations } from "./mutations";
import { actionView, canDecide, consequence, statusView, titleOf } from "./model";
import type { People } from "./people";
import { BannerView, ChangeBlock, Facts, Meta, PathsBlock, Section, SettingsBlock } from "./ProposalBody";
import { proposalQuery } from "./queries";

// The proposal drawer (Approvals page, 600 px, a dialog that does not take the page from you): consequence first, then the
// proposal as the server sent it, then the decision. It is a view of one `getProposal` answer, kept fresh by `proposal_changed`,
// so a proposal someone else decides while it is open says so, and its buttons go. The decision is the server's: the drawer
// sends it once, shows what comes back (the audit event, "sent for approval", a conflict, a lock, a refusal) and asks for the
// proposal and the queue again; it never marks anything decided before the server has said so.

type Props = {
  id: number;
  /** The proposal as the queue already holds it, so the drawer has something to show while its own answer is on the way. */
  initial: Proposal | undefined;
  me: string | null;
  people: People;
  onClose: () => void;
  /** Ask for the queue again: after a refusal that means it is not as it was. */
  onStale: () => void;
};

export function ProposalDrawer({ id, initial, me, people, onClose, onStale }: Props) {
  const query = useQuery({ ...proposalQuery(id), ...(initial === undefined ? {} : { placeholderData: initial }) });
  const state = useQueryState(query);
  const heading = useRef<HTMLHeadingElement>(null);
  useFocusOnOpen(heading);
  const p = state.data;
  return (
    <div role="dialog" aria-label="Proposal detail" className="absolute inset-y-0 right-0 z-30 flex w-[600px] max-w-full flex-col border-l border-border bg-popover text-popover-foreground shadow-popover">
      <div className="flex flex-none items-start gap-2 border-b border-border py-3 pr-3 pl-4">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          {p === undefined ? null : <Badges p={p} />}
          <h2 ref={heading} tabIndex={-1} className="text-base leading-[22px] font-semibold tracking-[-0.01em] outline-none">
            <SafeText text={p === undefined ? `Proposal AP-${id}` : titleOf(p)} />
          </h2>
          {p === undefined ? null : (
            <div className="flex flex-wrap items-center gap-x-2 font-mono text-xs leading-[18px] text-muted-foreground">
              <SafeText text={p.swimlane} className="text-foreground" />
              <SafeText text={p.action === "restart" ? "services" : p.path} />
            </div>
          )}
        </div>
        <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
          <X />
        </Button>
      </div>
      <QueryBoundaryView state={state} noun="proposal" source="the proposal" className="flex min-h-0 flex-1 flex-col">
        {(proposal) => <Detail key={proposal.id} p={proposal} me={me} people={people} onStale={onStale} refetch={state.refetch} />}
      </QueryBoundaryView>
    </div>
  );
}

function Badges({ p }: { p: Proposal }) {
  const action = actionView(p);
  const status = statusView(p.status);
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <Chip tone={action.tone} Icon={action.Icon}>
        {action.label}
      </Chip>
      <Chip tone={status.tone} Icon={status.Icon}>
        {status.label}
      </Chip>
      <ProposalNumber id={p.id} />
    </div>
  );
}

/** "AP-2046" with its two copy buttons (the number, and a link to the proposal), both shown when the number is hovered or focused. */
function ProposalNumber({ id }: { id: number }) {
  const label = useRef<HTMLSpanElement>(null);
  return (
    <span className={`${COPY_GROUP} inline-flex items-center gap-0.5`}>
      <span ref={label} className="inline-flex h-5 items-center rounded-sm border border-border px-1.5 font-mono text-xs">
        AP-{id}
      </span>
      <CopyButton value={`AP-${id}`} what="proposal ID" source={label} className={COPY_REVEAL} />
      <CopyLink href={linkTo("approval", { id: String(id) })} what="link to this proposal" className={COPY_REVEAL} />
    </span>
  );
}

const heads = (p: Proposal): string => (p.status === "expired" ? "What it would have done" : p.status === "pending" || p.status === "stale" ? "If you approve" : "Result");

/** The paths worth their own section: a restart's services, a pull request's files, anything that names more than one. */
const namesPaths = (p: Proposal): boolean => p.action === "restart" || p.action === "pr" || p.action === "notify" || (p.paths?.length ?? 0) > 1;

function Detail({ p, me, people, onStale, refetch }: { p: Proposal; me: string | null; people: People; onStale: () => void; refetch: () => void }) {
  const client = useQueryClient();
  const now = useNow(true, 30_000);
  const approve = useMutation(approvalMutations.approve(client));
  const reject = useMutation(approvalMutations.reject(client));
  const [reason, setReason] = useState("");
  const base = useWriteGate("admin");
  const decide = canDecide(p, me);

  // Did the drawer show this proposal while it was pending? A decision that arrives later was made while the person had it open.
  const sawPending = useRef(p.status === "pending");
  useEffect(() => {
    if (p.status === "pending") sawPending.current = true;
  }, [p.status]);

  const answer = useMemo<Answer | null>(() => {
    const data = approve.data;
    if (data === undefined) return null;
    return data.outcome === "proposal_created" ? { kind: "sent", id: data.id } : { kind: "approved", audit: data.audit_id, warnings: data.warnings?.length ?? 0 };
  }, [approve.data]);

  const others = useMemo(() => people.otherAdmins(me).map((u) => u.display_name), [people, me]);
  const banner = bannerOf({ p, me, name: people.name, others, now, answer, sawPending: sawPending.current });

  const failed: ApiProblem | null = approve.error ?? reject.error;
  const dialog = dialogOf(failed);
  const own = me !== null && p.author === me;
  const inline = failed !== null && !approve.isPending && !reject.isPending ? failureOf(failed, { verb: approve.error === null ? "reject" : "approve", own }) : null;
  const working = approve.isPending ? "approve" : reject.isPending ? "reject" : null;

  // A refusal that says the proposal is not as it was (not pending any more, not there, not yours to decide) means what is on
  // screen is old: ask for the proposal and the queue again. A conflict and a lock refetch by themselves (src/lib/query/invalidate.ts).
  const stale = (problem: ApiProblem): void => {
    if (problem.status === 403 || problem.status === 404 || problem.status === 422) {
      refetch();
      onStale();
    }
  };
  const doApprove = (): void => {
    reject.reset();
    // The path matters to the file's caches only for a decision that writes the file; a restart or a pull request changes none.
    const writesFile = p.action === undefined || p.action === "edit" || p.action === "upload" || p.action === "delete" || p.action === "revert";
    approve.mutate({ id: p.id, swimlane: p.swimlane, ...(writesFile ? { path: p.path } : {}) }, { onError: stale });
  };
  const doReject = (): void => {
    approve.reset();
    reject.mutate({ id: p.id, body: { reason: reason.trim() } }, { onError: stale });
  };
  const closeDialog = (): void => {
    approve.reset();
    reject.reset();
  };

  const showChange = p.hunks.length > 0 || p.action === "edit" || p.action === "upload" || p.action === "revert";
  const authorName = people.name(p.author);
  const conflict = failed !== null && failed.status === 409 ? failed : null;
  const until = failed !== null && typeof failed.body["until"] === "number" ? failed.body["until"] : null;
  const current = conflict !== null && typeof conflict.body["current"] === "string" ? conflict.body["current"] : null;

  return (
    <>
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3.5">
        <Meta p={p} people={people} now={now} />
        {banner !== null ? <BannerView banner={banner} /> : null}
        <Section title={heads(p)}>
          <p className="text-sm leading-5">
            <SafeText text={consequence(p, p.swimlane)} />
          </p>
        </Section>
        {showChange ? (
          <Section title="Proposed change">
            <ChangeBlock p={p} />
          </Section>
        ) : null}
        {p.setting_changes.length > 0 ? (
          <Section title="Settings changed">
            <SettingsBlock changes={p.setting_changes} />
          </Section>
        ) : null}
        {namesPaths(p) ? (
          <Section title={p.action === "restart" ? "Services" : "Files"}>
            <PathsBlock p={p} />
          </Section>
        ) : null}
        {p.action === "delete" ? (
          <Section title="The file">
            <Facts p={p} />
          </Section>
        ) : null}
      </div>
      <DecisionFooter
        p={p}
        authorName={authorName}
        decide={decide}
        base={base}
        reason={reason}
        onReason={setReason}
        working={working}
        onApprove={doApprove}
        onReject={doReject}
        error={inline}
      />
      <ProblemDialog
        open={dialog === "conflict"}
        onOpenChange={(open) => {
          if (!open) closeDialog();
        }}
        variant="conflict"
        title="The file changed first"
        description={`Nothing was written. The file on ${p.swimlane} changed after this was proposed, so the proposal is now stale and can't be approved.`}
        {...(conflict?.requestId === undefined ? {} : { requestId: conflict.requestId })}
      >
        {current !== null ? (
          <p className="flex items-center gap-1.5 text-sm">
            The file now has hash <HashChip hash={current} />.
          </p>
        ) : null}
      </ProblemDialog>
      <ProblemDialog
        open={dialog === "locked"}
        onOpenChange={(open) => {
          if (!open) closeDialog();
        }}
        variant="locked"
        description={`${p.swimlane} is locked, so nothing was approved. Try again when the lock clears.`}
        {...(failed?.requestId === undefined ? {} : { requestId: failed.requestId })}
      >
        {until !== null ? <p className="text-sm">Locked until {formatTime(until)}.</p> : null}
      </ProblemDialog>
    </>
  );
}
