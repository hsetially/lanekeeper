import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState, type SyntheticEvent } from "react";
import { Role } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { ROLE_ORDER, roleLabel } from "@/auth/roles";
import { GatedButton } from "@/components/app/GatedButton";
import { useWriteGate } from "@/components/app/write-gate";
import { WriteError } from "@/components/app/WriteError";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";
import { Hourglass } from "@/icons";
import { accessMutations } from "./mutations";
import { writeErrorInfo } from "./problem-info";

// "Request a role change": the one next step of the permission state in the States index ("Admin needs the Admin role. You are
// signed in as Viewer ... [Request a role change]"), and the contract's `createAccessRequest`. A person who has a role asks an
// Admin for a higher one and says why; the Admin decides in Admin > Users and access requests. Mounted by whatever screen shows a
// permission state (`<PermissionState action={<RequestRoleButton .../>}>`).
//
// A write, so it shows nothing as done until the hub answers (never optimistic); the answer is shown in this dialog: 202 "request
// sent", or the reason it was not (409 an open request is waiting or the role is the one the person has, 429 too soon).

const MAX_REASON = 256;

const ROLE_HINT: Record<Role, string> = {
  [Role.viewer]: "Read everything.",
  [Role.editor]: "Save files and raise pull requests.",
  [Role.operator]: "Everything an Editor can do, and restart or notify services.",
  [Role.admin]: "Everything an Operator can do, and manage users, swimlanes and approvals.",
};

/** The roles a person can ask for: the ones above theirs, the lowest first. A person with no role cannot ask (they are not active yet). */
export function rolesAbove(have: Role | null): Role[] {
  if (have === null) return [];
  return ROLE_ORDER.slice(ROLE_ORDER.indexOf(have) + 1);
}

function firstRole(options: readonly Role[], need: Role | undefined): Role | undefined {
  return need !== undefined && options.includes(need) ? need : options[0];
}

function statusSentences(error: unknown): Partial<Record<number, string>> {
  const wait = error instanceof ApiProblem ? error.retryAfterSeconds : undefined;
  return {
    400: "The hub did not accept that request. Check the role and the reason.",
    403: "Your session is not allowed to ask for this. Sign in again, or ask an Admin directly.",
    409: "You already have an open request, or you asked for the role you have. Wait for an Admin to decide.",
    429: `You asked recently. ${wait === undefined ? "Try again later." : `Try again in ${wait} second${wait === 1 ? "" : "s"}.`}`,
  };
}

type DialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  have: Role | null;
  /** The role the screen asked for, if it did. */
  need?: Role;
};

export function RequestRoleDialog({ open, onOpenChange, have, need }: DialogProps) {
  const client = useQueryClient();
  const request = useMutation(accessMutations.requestRole(client));
  // A write: it needs the live connection (any signed-in role may ask).
  const gate = useWriteGate("viewer");
  const options = rolesAbove(have);
  const [role, setRole] = useState<Role | undefined>(() => firstRole(options, need));
  const [reason, setReason] = useState("");
  const [reasonError, setReasonError] = useState<string | null>(null);

  const working = request.isPending;
  const sent = request.data;

  const close = (next: boolean): void => {
    if (!next && working) return;
    onOpenChange(next);
    if (!next) {
      request.reset();
      setReason("");
      setReasonError(null);
      setRole(firstRole(options, need));
    }
  };

  const submit = (event: SyntheticEvent): void => {
    event.preventDefault();
    if (working || role === undefined || !gate.allowed) return;
    const trimmed = reason.trim();
    if (trimmed === "") {
      setReasonError("Say why you need it.");
      return;
    }
    setReasonError(null);
    request.mutate({ requested_role: role, reason: trimmed });
  };

  const asked = sent?.requested_role ?? role;

  return (
    <AlertDialog open={open} onOpenChange={close}>
      <AlertDialogContent size="md">
        {sent ? (
          <>
            <AlertDialogHeader>
              <AlertDialogTitle className="flex items-center gap-2">
                <Hourglass aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
                Request sent
              </AlertDialogTitle>
              <AlertDialogDescription>
                {asked === undefined ? "An Admin will decide on your request." : `An Admin will decide on your request for the ${roleLabel(asked)} role.`} You keep your current role until they do.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Close</AlertDialogCancel>
            </AlertDialogFooter>
          </>
        ) : (
          <form onSubmit={submit} noValidate className="flex flex-col gap-3">
            <AlertDialogHeader>
              <AlertDialogTitle>Request a role change</AlertDialogTitle>
              <AlertDialogDescription>An Admin sees your request and the reason, and decides. Your role does not change until they do.</AlertDialogDescription>
            </AlertDialogHeader>

            <div className="flex flex-col gap-1.5">
              <span id="request-role-label" className="text-sm font-medium">
                Role you need
              </span>
              <RadioGroup aria-labelledby="request-role-label" value={role ?? ""} onValueChange={(next) => { setRole(next as Role); }} disabled={working}>
                {options.map((option) => (
                  <Label key={option} className="items-start gap-2 font-normal">
                    <RadioGroupItem value={option} className="mt-0.5" />
                    <span className="flex flex-col">
                      <span className="font-medium">{roleLabel(option)}</span>
                      <span className="text-xs text-muted-foreground">{ROLE_HINT[option]}</span>
                    </span>
                  </Label>
                ))}
              </RadioGroup>
            </div>

            <Field invalid={reasonError !== null} disabled={working}>
              <FieldLabel>Why you need it</FieldLabel>
              <FieldControl>
                <Textarea
                  value={reason}
                  maxLength={MAX_REASON}
                  onChange={(event) => {
                    setReason(event.target.value);
                    if (reasonError) setReasonError(null);
                  }}
                />
              </FieldControl>
              {reasonError ? <FieldError>{reasonError}</FieldError> : <FieldDescription>The Admin who decides reads this. At most {MAX_REASON} characters.</FieldDescription>}
            </Field>

            {request.error ? <WriteError {...writeErrorInfo(request.error, "The request could not be sent", statusSentences(request.error))} /> : null}

            <AlertDialogFooter>
              <AlertDialogCancel disabled={working}>Cancel</AlertDialogCancel>
              <GatedButton type="submit" minRole="viewer" loading={working}>
                {working ? "Sending…" : "Send request"}
              </GatedButton>
            </AlertDialogFooter>
          </form>
        )}
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** The button that opens the dialog. Not drawn when there is no role to ask for. */
export function RequestRoleButton({ have, need, children = "Request a role change" }: { have: Role | null; need?: Role; children?: string }) {
  const [open, setOpen] = useState(false);
  if (rolesAbove(have).length === 0) return null;
  return (
    <>
      <Button
        variant="secondary"
        onClick={() => {
          setOpen(true);
        }}
      >
        {children}
      </Button>
      <RequestRoleDialog open={open} onOpenChange={setOpen} have={have} {...(need === undefined ? {} : { need })} />
    </>
  );
}
