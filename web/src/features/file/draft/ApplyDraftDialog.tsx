import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type { Draft } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { HashChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { formatTime } from "@/lib/time";
import type { WriteOutcome } from "../edit/outcome";
import { useWriteDialog } from "../edit/useWriteDialog";
import { fileMutations } from "../mutations";

// Apply a draft (Draft, review): the consequence first, then what is applied and to what. A draft is a write like any other:
// it carries the hash the file had when the draft was made, so a file that changed since is a conflict and nothing is written;
// it may be sent for approval, and it may be locked. Those go to the page; a refusal or a failure stays here, inline, with the
// request id. Nothing is optimistic.

export type ApplyDraftDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  draft: Draft;
  onSettled: (outcome: WriteOutcome) => void;
};

export function ApplyDraftDialog({ open, onOpenChange, draft, onSettled }: ApplyDraftDialogProps) {
  const client = useQueryClient();
  const mutation = useMutation(fileMutations.applyDraft(client));
  const write = useWriteDialog(mutation, (outcome) => {
    onOpenChange(false);
    onSettled(outcome);
  });
  const { clear } = write;
  useEffect(() => {
    if (!open) clear();
  }, [open, clear]);

  return (
    <ConsequenceDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Apply this draft?"
      consequence={`This writes the draft's version of the file to ${draft.swimlane} NFS now. It is applied only if the file is still as it was when the draft was made.`}
      actionLabel="Apply draft"
      workingLabel="Applying…"
      working={write.working}
      error={write.error}
      onConfirm={() => {
        void write.run({ draft: draft.id, swimlane: draft.swimlane, path: draft.path });
      }}
    >
      <p className="font-mono text-xs text-muted-foreground">
        <SafeText text={draft.path} />
      </p>
      <dl className="grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
        <dt className="text-muted-foreground">Based on</dt>
        <dd className="m-0">{draft.expected.state === "hash" ? <HashChip hash={draft.expected.hash} /> : <span>No file: this creates it</span>}</dd>
        <dt className="text-muted-foreground">New version</dt>
        <dd className="m-0">
          <HashChip hash={draft.new_hash} />
        </dd>
        <dt className="text-muted-foreground">Kept until</dt>
        <dd className="m-0">{formatTime(draft.expires_at)}</dd>
      </dl>
    </ConsequenceDialog>
  );
}
