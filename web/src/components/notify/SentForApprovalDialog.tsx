import type { RefObject } from "react";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { AppLink } from "@/routes/AppLink";
import { OutcomeDialog } from "./OutcomeDialog";

// A write that was sent for approval is not done: the server answered `proposal_created`, so nothing changed yet. The dialog says
// what was sent, names the request, and offers the request only to someone who can open it (an Admin); the others are told who
// decides. `description` is the screen's own sentence ("Nothing restarts until an Admin approves request AP-12."). It is the
// approval kind of OutcomeDialog, so that focus can go back to the button that started the flow when it closes.

type Props = {
  open: boolean;
  title: string;
  description: string;
  /** What was sent, as a person reads it: "config-server in sitb". Untrusted text. */
  subject: string;
  request: number;
  canOpenRequest: boolean;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
};

export function SentForApprovalDialog({ open, title, description, subject, request, canOpenRequest, onClose, returnFocusRef }: Props) {
  return (
    <OutcomeDialog
      open={open}
      onClose={onClose}
      tone="approval"
      title={title}
      description={description}
      actions={
        canOpenRequest ? (
          <Button asChild variant="secondary">
            <AppLink route="approval" params={{ id: String(request) }}>
              View request
            </AppLink>
          </Button>
        ) : null
      }
      {...(returnFocusRef ? { returnFocusRef } : {})}
    >
      <p className="text-sm text-muted-foreground">
        <SafeText text={subject} className="font-mono" />, request AP-{request}.
      </p>
    </OutcomeDialog>
  );
}
