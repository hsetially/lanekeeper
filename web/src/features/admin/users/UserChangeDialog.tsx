import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Role, User } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { visibleText } from "@/components/app/safe-text";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { roleLabel } from "@/auth/roles";
import { adminMutations } from "@/features/admin/mutations";
import { roleNote, roleWord } from "@/features/admin/roles";
import { adminWriteError } from "@/features/admin/write-error";
import { adminKeys } from "@/lib/query/keys";
import { invalidateTargets } from "@/lib/query/targets";

// The changes to a person that name their consequence first: a new role, disabling, and no longer requiring approval. (Enabling and
// requiring approval are not consequences to fear and are made at once, see People.tsx.) The write is not optimistic: the dialog
// stays open and busy until the hub has answered and the list has been read again, and an answer that refuses is shown in the
// dialog with the choice intact (409: "This user changed while you were editing", state C03 of the design).

export type UserChange =
  | { readonly kind: "role"; readonly user: User; readonly to: Role }
  | { readonly kind: "disable"; readonly user: User }
  | { readonly kind: "approval-off"; readonly user: User };

type Words = { title: string; consequence: string; action: string; working: string };

function words(change: UserChange | null): Words {
  if (change === null) return { title: "Change user", consequence: "", action: "Save", working: "Saving…" };
  const name = visibleText(change.user.display_name);
  switch (change.kind) {
    case "role":
      return {
        title: `Change ${name}'s role`,
        consequence: `${name} changes from ${roleWord(change.user.role)} to ${roleLabel(change.to)}. ${roleNote(change.to)}`,
        action: "Change role",
        working: "Changing role…",
      };
    case "disable":
      return {
        title: `Disable ${name}`,
        consequence: `${name} can no longer sign in or use Lanekeeper until an Admin enables them again. Their history stays in the audit log.`,
        action: "Disable user",
        working: "Disabling…",
      };
    case "approval-off":
      return {
        title: `Stop requiring approval for ${name}`,
        consequence: `Their changes take effect without an Admin's approval from now on.`,
        action: "Stop requiring approval",
        working: "Saving…",
      };
  }
}

function bodyOf(change: UserChange): { role: Role } | { status: "disabled" } | { requires_approval: false } {
  if (change.kind === "role") return { role: change.to };
  if (change.kind === "disable") return { status: "disabled" };
  return { requires_approval: false };
}

const CONFLICT: WriteErrorInfo = {
  title: "This user changed while you were editing",
  detail: "Nothing was applied. Another Admin may have changed this person, or this change would leave no active Admin. Reload to see the current state before you change them again.",
};

export function UserChangeDialog({ change, onClose }: { change: UserChange | null; onClose: () => void }) {
  const client = useQueryClient();
  const update = useMutation(adminMutations.updateUser(client));
  const text = words(change);
  const conflict = update.error?.status === 409;
  const error: WriteErrorInfo | null =
    update.error === null ? null : conflict ? { ...CONFLICT, ...(update.error.requestId === undefined ? {} : { requestId: update.error.requestId }) } : adminWriteError(update.error, "The change could not be made");

  const close = (): void => {
    update.reset();
    onClose();
  };
  return (
    <ConsequenceDialog
      open={change !== null}
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={text.title}
      consequence={text.consequence}
      actionLabel={conflict ? "Reload user" : text.action}
      workingLabel={text.working}
      cancelLabel={conflict ? "Keep editing" : "Cancel"}
      working={update.isPending}
      error={error}
      onConfirm={() => {
        if (change === null) return;
        if (conflict) {
          // The write already read the people again (a 409 means they are out of date); this asks once more and lets go.
          void invalidateTargets(client, [{ kind: "key", key: adminKeys.section("users") }]);
          close();
          return;
        }
        update.mutate({ id: change.user.id, body: bodyOf(change), self: false }, { onSuccess: close });
      }}
    />
  );
}
