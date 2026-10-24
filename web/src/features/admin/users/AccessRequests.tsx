import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { AccessRequest, Role } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { GatedButton } from "@/components/app/GatedButton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldError, FieldLabel } from "@/components/ui/field";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { roleLabel } from "@/auth/roles";
import { adminMutations } from "@/features/admin/mutations";
import { ROLE_CHOICES, roleNote } from "@/features/admin/roles";
import { adminWriteError } from "@/features/admin/write-error";
import { usePendingRequests } from "@/features/admin/users/pending";
import { Check, CircleCheck, Inbox, X } from "@/icons";
import { formatRelative } from "@/lib/time";
import { useNow } from "@/live/use-now";

// Pending access requests: a person signed in and has no role yet, and waits here for an Admin. Approve (with the role, which the
// Admin chooses; the one asked for is offered) and reject (with a reason) are two writes that are not optimistic: each names its
// consequence in a dialog first, the dialog stays until the hub answers, and the request leaves the list when the list is read
// again. The list is `access_request_created`'s: a new request appears without a reload.

type Outcome = { readonly kind: "approved"; readonly email: string; readonly role: Role } | { readonly kind: "rejected"; readonly email: string };
type Deciding = { readonly kind: "approve"; readonly request: AccessRequest; readonly role: Role } | { readonly kind: "reject"; readonly request: AccessRequest };

const MAX_REASON = 256;

/** `PS` for priya.shah@example.test: from the local part of an address, which is all an access request has (UserMenu's `initials` reads a display name). */
function emailInitials(email: string): string {
  const local = email.split("@")[0] ?? "";
  const parts = local.split(/[._-]+/).filter((part) => part !== "");
  const letters = parts.length >= 2 ? [parts[0], parts[1]] : [local.slice(0, 2)];
  return letters
    .map((part) => (part ?? "").charAt(0).toUpperCase())
    .join("")
    .slice(0, 2);
}

function RequestCard({ request, now, onApprove, onReject }: { request: AccessRequest; now: number; onApprove: (request: AccessRequest, role: Role) => void; onReject: (request: AccessRequest) => void }) {
  const [role, setRole] = useState<Role | "">(request.requested_role ?? "");
  const [missing, setMissing] = useState(false);
  const email = visibleText(request.email);
  return (
    <div role="group" aria-label={`Access request from ${email}`} className="flex flex-wrap items-center gap-x-6 gap-y-3 rounded-md border border-border bg-card px-4 py-3">
      <div className="min-w-0 flex-1 basis-[280px]">
        <div className="flex items-center gap-2.5">
          <span aria-hidden="true" className="inline-flex size-8 shrink-0 items-center justify-center rounded-full border border-input bg-muted text-xs font-semibold">
            {emailInitials(request.email)}
          </span>
          <b className="min-w-0 truncate text-sm font-semibold">
            <SafeText text={request.email} />
          </b>
        </div>
        <p className="mt-2 flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
          <span>Signed in · requested {formatRelative(request.created_at, now)}</span>
          <span>·</span>
          <span>{request.requested_role === null ? "Asked for no role in particular" : `Asked for ${roleLabel(request.requested_role)}`}</span>
        </p>
        {request.reason === null || request.reason === "" ? null : (
          <p className="mt-1.5 text-sm text-pretty">
            “<SafeText text={request.reason} />”
          </p>
        )}
      </div>
      <div className="flex max-w-full flex-none flex-col items-start gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-muted-foreground">Approve as</span>
          <div className="w-33">
            <NativeSelect
              aria-label={`Approve ${email} as`}
              aria-invalid={missing || undefined}
              value={role}
              onChange={(event) => {
                setRole(event.target.value as Role | "");
                setMissing(false);
              }}
            >
              <option value="">Choose a role</option>
              {ROLE_CHOICES.map((choice) => (
                <option key={choice} value={choice}>
                  {roleLabel(choice)}
                </option>
              ))}
            </NativeSelect>
          </div>
          <GatedButton
            minRole="admin"
            aria-label={`Approve request from ${email}`}
            onClick={() => {
              if (role === "") {
                setMissing(true);
                return;
              }
              onApprove(request, role);
            }}
          >
            <Check data-icon="inline-start" />
            Approve
          </GatedButton>
          <GatedButton
            minRole="admin"
            variant="secondary"
            aria-label={`Reject request from ${email}`}
            onClick={() => {
              onReject(request);
            }}
          >
            Reject
          </GatedButton>
        </div>
        {missing ? (
          <p role="alert" className="flex items-center gap-1 text-xs text-danger-fg">
            <X aria-hidden="true" className="size-3" />
            Choose a role before approving
          </p>
        ) : null}
        {role === "" ? null : <p className="max-w-[420px] text-xs text-muted-foreground">{roleNote(role)}</p>}
      </div>
    </div>
  );
}

function DecisionDialog({ deciding, onClose, onDone }: { deciding: Deciding | null; onClose: () => void; onDone: (outcome: Outcome) => void }) {
  const client = useQueryClient();
  const approve = useMutation(adminMutations.approveAccessRequest(client));
  const reject = useMutation(adminMutations.rejectAccessRequest(client));
  const [reason, setReason] = useState("");
  const [reasonError, setReasonError] = useState<string | null>(null);
  const mutation = deciding?.kind === "reject" ? reject : approve;
  const request = deciding?.request;
  const email = visibleText(request?.email ?? "");

  const close = (): void => {
    approve.reset();
    reject.reset();
    setReason("");
    setReasonError(null);
    onClose();
  };
  const error = mutation.error === null ? null : adminWriteError(mutation.error, deciding?.kind === "reject" ? "The request could not be rejected" : "The request could not be approved", { 409: "This request was already decided. The list has been refreshed." });

  if (deciding?.kind === "reject") {
    return (
      <ConsequenceDialog
        open
        onOpenChange={(open) => {
          if (!open) close();
        }}
        title={`Reject ${email}'s request`}
        consequence="They stay signed out and can request access again."
        actionLabel="Reject request"
        workingLabel="Rejecting…"
        working={mutation.isPending}
        error={error}
        onConfirm={() => {
          const trimmed = reason.trim();
          if (trimmed === "") {
            setReasonError("Give a reason");
            return;
          }
          reject.mutate(
            { id: deciding.request.id, body: { reason: trimmed } },
            {
              onSuccess: () => {
                onDone({ kind: "rejected", email: deciding.request.email });
                close();
              },
            },
          );
        }}
      >
        <Field invalid={reasonError !== null}>
          <FieldLabel>Reason</FieldLabel>
          <FieldControl>
            <Textarea
              value={reason}
              maxLength={MAX_REASON}
              rows={3}
              onChange={(event) => {
                setReason(event.target.value);
                setReasonError(null);
              }}
            />
          </FieldControl>
          <FieldError>{reasonError}</FieldError>
        </Field>
      </ConsequenceDialog>
    );
  }
  return (
    <ConsequenceDialog
      open={deciding !== null}
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={deciding === null ? "Approve request" : `Approve ${email} as ${roleLabel(deciding.role)}`}
      consequence={deciding === null ? "" : `${email} can sign in as ${roleLabel(deciding.role)} straight away. ${roleNote(deciding.role)}`}
      actionLabel="Approve request"
      workingLabel="Approving…"
      working={mutation.isPending}
      error={error}
      onConfirm={() => {
        if (deciding === null) return;
        approve.mutate(
          { id: deciding.request.id, body: { role: deciding.role } },
          {
            onSuccess: () => {
              onDone({ kind: "approved", email: deciding.request.email, role: deciding.role });
              close();
            },
          },
        );
      }}
    />
  );
}

export function AccessRequests() {
  const { pages, state } = usePendingRequests();
  const now = useNow(true, 60_000);
  const [deciding, setDeciding] = useState<Deciding | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const count = state.data?.length ?? 0;
  // A request the person decided stays out of the list the moment the list is read again; the list may still hold more pages.
  const badge = state.state === "loading" || state.state === "error" ? null : count === 0 ? "None waiting" : `${count}${pages.hasMore ? "+" : ""} waiting`;

  return (
    <section aria-label="Pending requests" className="flex flex-col gap-2">
      <div className="flex min-h-6 items-center gap-2">
        <h3 className="text-sm font-semibold">Pending requests</h3>
        {badge === null ? null : <Badge variant="approval">{badge}</Badge>}
      </div>
      {outcome ? (
        <Alert variant={outcome.kind === "approved" ? "success" : "info"} icon={outcome.kind === "approved" ? <CircleCheck aria-hidden="true" /> : undefined}>
          <AlertTitle>{outcome.kind === "approved" ? `${visibleText(outcome.email)} approved as ${roleLabel(outcome.role)}` : `Request from ${visibleText(outcome.email)} rejected`}</AlertTitle>
          <AlertDescription>{outcome.kind === "approved" ? "They can sign in now. The change is in the audit log." : "They stay signed out and can request access again. The change is in the audit log."}</AlertDescription>
        </Alert>
      ) : null}
      <QueryBoundaryView
        state={state}
        noun="access requests"
        source="the access requests"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={Inbox} title="No pending requests">
              New sign-ins that need approval appear here.
            </EmptyState>
          </div>
        }
      >
        {(items) => (
          <div className="flex flex-col gap-2">
            {items.map((request) => (
              <RequestCard
                key={request.id}
                request={request}
                now={now}
                onApprove={(r, role) => {
                  setOutcome(null);
                  setDeciding({ kind: "approve", request: r, role });
                }}
                onReject={(r) => {
                  setOutcome(null);
                  setDeciding({ kind: "reject", request: r });
                }}
              />
            ))}
            {pages.hasMore ? (
              <div>
                <Button variant="secondary" size="sm" onClick={() => void pages.loadMore()}>
                  Load more requests
                </Button>
              </div>
            ) : null}
          </div>
        )}
      </QueryBoundaryView>
      <DecisionDialog
        deciding={deciding}
        onClose={() => {
          setDeciding(null);
        }}
        onDone={setOutcome}
      />
    </section>
  );
}
