import type { UseMutationResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { ApiProblem } from "@/api/problem";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { GatedButton } from "@/components/app/GatedButton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { adminWriteError } from "@/features/admin/write-error";
import { CircleCheck, Pencil, Plus, Trash2 } from "@/icons";

// The parts that the rule lists share (path mappings, merge modes, host tiers, folder mappings, notification channels): a header with
// the count and the button that adds one, the edit and delete buttons of a row, the dialog that confirms a delete, and the notice
// that says a rule was saved. Each is a write, so each button is a GatedButton (disabled with the reason, never hidden), and each
// dialog names its consequence and stays until the hub has answered.

export function RulesHeader({ title, count, addLabel, onAdd }: { title: string; count: string; addLabel: string; onAdd: () => void }) {
  return (
    <div className="flex min-h-7 items-center gap-2">
      <h3 className="text-sm font-semibold">{title}</h3>
      <span className="text-xs text-muted-foreground">{count}</span>
      <GatedButton
        minRole="admin"
        variant="secondary"
        className="ml-auto"
        onClick={() => {
          onAdd();
        }}
      >
        <Plus data-icon="inline-start" />
        {addLabel}
      </GatedButton>
    </div>
  );
}

/** Edit and delete for one row. `what` names the row for assistive technology ("rule security"). */
export function RuleActions({ what, onEdit, onDelete }: { what: string; onEdit: () => void; onDelete: () => void }) {
  return (
    <span className="flex items-center justify-end gap-1">
      <GatedButton
        minRole="admin"
        variant="ghost"
        size="icon-sm"
        aria-label={`Edit ${what}`}
        onClick={() => {
          onEdit();
        }}
      >
        <Pencil />
      </GatedButton>
      <GatedButton
        minRole="admin"
        variant="ghost"
        size="icon-sm"
        aria-label={`Delete ${what}`}
        onClick={() => {
          onDelete();
        }}
      >
        <Trash2 />
      </GatedButton>
    </span>
  );
}

export type DeleteTarget = { readonly id: string; readonly what: string };

/** Confirms the deletion of one rule: what it changes, and the hub's answer if it refuses. The row goes when the list is read again. */
export function DeleteRuleDialog({
  target,
  title,
  consequence,
  action = "Delete rule",
  mutation,
  onClose,
  onDone,
}: {
  target: DeleteTarget | null;
  title: string;
  consequence: string;
  /** The verb on the button. */
  action?: string;
  mutation: UseMutationResult<unknown, ApiProblem, string>;
  onClose: () => void;
  onDone?: (target: DeleteTarget) => void;
}) {
  return (
    <ConsequenceDialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) {
          mutation.reset();
          onClose();
        }
      }}
      title={title}
      consequence={consequence}
      actionLabel={action}
      workingLabel="Deleting…"
      working={mutation.isPending}
      error={mutation.error === null ? null : adminWriteError(mutation.error, "The rule could not be deleted")}
      onConfirm={() => {
        if (target === null) return;
        mutation.mutate(target.id, {
          onSuccess: () => {
            onDone?.(target);
            mutation.reset();
            onClose();
          },
        });
      }}
    />
  );
}

/** "Saved": what was written, said after the dialog has closed. It is the screen's, not a toast, because it is the person's own write. */
export function SavedNotice({ title, children, onDismiss }: { title: string; children?: ReactNode; onDismiss: () => void }) {
  return (
    <Alert variant="success" icon={<CircleCheck aria-hidden="true" />}>
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <AlertTitle>{title}</AlertTitle>
          {children ? <AlertDescription>{children}</AlertDescription> : null}
        </div>
        <Button variant="ghost" size="sm" onClick={onDismiss}>
          Dismiss
        </Button>
      </div>
    </Alert>
  );
}
