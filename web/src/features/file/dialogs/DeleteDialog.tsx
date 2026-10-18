import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import { SafeText } from "@/components/app/SafeText";
import { WriteError } from "@/components/app/WriteError";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { TriangleAlert, Trash2 } from "@/icons";
import { classify, errorInfo, type WriteOutcome } from "../edit/outcome";
import { useWriteDialog } from "../edit/useWriteDialog";
import { fileMutations } from "../mutations";
import { EffectPreview } from "./EffectPreview";

// Delete a file (Consequence dialogs, 2a and 2b). The effect first, from the engine, as soon as the dialog opens; then the file's
// name typed in full. It is a DestructiveConfirm, so it follows its rules (initial focus on Cancel, the destructive button stays
// off until the name matches, Enter never confirms), with the effect table in the slot the design gives it, between the
// consequence and the field, and the button also waiting for the effect to be known.
//
// The delete carries the preview's token and the hash it expects: a file that changed under the person is a conflict and
// nothing is deleted, and a delete with no valid preview is refused by the server.

export type DeleteDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  lane: string;
  path: string;
  name: string;
  /** The hash of the file as the person saw it. */
  baseHash: string;
  onSettled: (outcome: WriteOutcome) => void;
};

export function DeleteDialog({ open, onOpenChange, lane, path, name, baseHash, onSettled }: DeleteDialogProps) {
  const client = useQueryClient();
  const preview = useMutation(fileMutations.preview(client));
  const remove = useMutation(fileMutations.delete(client));
  const write = useWriteDialog(remove, (outcome) => {
    onOpenChange(false);
    onSettled(outcome);
  });
  const { mutate, reset } = preview;
  const { clear } = write;
  const asked = useRef(false);

  const ask = () => mutate({ swimlane: lane, body: { action: "delete", path, expected_state: "hash", expected_hash: baseHash } });
  useEffect(() => {
    if (open && !asked.current) {
      asked.current = true;
      mutate({ swimlane: lane, body: { action: "delete", path, expected_state: "hash", expected_hash: baseHash } });
    }
    if (!open) {
      asked.current = false;
      reset();
      clear();
    }
  }, [open, mutate, reset, clear, lane, path, baseHash]);

  const known = preview.data !== undefined;
  const previewError = preview.error
    ? (() => {
        const outcome = classify({ error: preview.error });
        return outcome.kind === "invalid" || outcome.kind === "too_large" || outcome.kind === "failed" ? errorInfo(outcome) : { title: "The effect could not be worked out", detail: "Nothing was deleted." };
      })()
    : null;

  return (
    <DestructiveConfirm
      open={open}
      onOpenChange={onOpenChange}
      size="lg"
      title={`Delete ${name} from ${lane}?`}
      titleIcon={<TriangleAlert className="size-4" />}
      consequence={`The file leaves the ${lane} NFS share now. Services that already loaded it keep using it until they restart.`}
      confirmValue={name}
      actionLabel="Delete file"
      actionIcon={<Trash2 className="size-3.5" />}
      workingLabel="Deleting…"
      working={write.working}
      // A delete with no valid preview is refused by the server anyway; the button waits for the effect.
      canConfirm={known}
      showMatchHint
      error={write.error}
      onConfirm={() => {
        if (preview.data === undefined) return;
        void write.run({ swimlane: lane, params: { path, expected: baseHash, preview_id: preview.data.preview_id } });
      }}
    >
      <p className="font-mono text-xs text-muted-foreground">
        <SafeText text={path} />
      </p>
      {preview.isPending ? (
        <div role="status" aria-busy="true" aria-label="Working out what changes" className="flex flex-col gap-2">
          <Skeleton className="h-4 w-3/5" />
          <Skeleton className="h-4 w-4/5" />
          <Skeleton className="h-4 w-2/5" />
        </div>
      ) : null}
      {preview.data ? <EffectPreview preview={preview.data} /> : null}
      {previewError ? (
        <div className="flex flex-col gap-2">
          <WriteError {...previewError} />
          <div>
            <Button variant="secondary" size="sm" onClick={ask}>
              Try again
            </Button>
          </div>
        </div>
      ) : null}
      <p className="m-0 text-sm">The file stays in Git. The swimlane shows Git ahead until a PR removes it there.</p>
    </DestructiveConfirm>
  );
}
