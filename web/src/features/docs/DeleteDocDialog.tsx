import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type { DocSummary } from "@/api/generated/model";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import { docFileName } from "./doc-path";
import { docWriteError } from "./errors";
import { docMutations } from "./mutations";

// Delete an uploaded doc (Editor and above). The design draws this dialog for a file; a doc has no frame of its own, so it is the
// same dialog: say what is lost, ask for the file's name, and never confirm on Enter. The name to type is the file's, which is the
// last segment of the doc's path.

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  doc: DocSummary;
  /** The server deleted it. Called before the dialog closes. */
  onDone?: () => void;
};

export function DeleteDocDialog({ open, onOpenChange, doc, onDone }: Props) {
  const client = useQueryClient();
  const remove = useMutation(docMutations.delete(client));
  const { reset } = remove;
  useEffect(() => {
    if (!open) reset();
  }, [open, reset]);

  const error = remove.error;
  return (
    <DestructiveConfirm
      open={open}
      onOpenChange={onOpenChange}
      title={`Delete “${doc.title}”?`}
      consequence="Removes the doc for everyone. Links to it stop working, and it no longer turns up in search. This app has no undo for it."
      confirmValue={docFileName(doc.path)}
      actionLabel="Delete doc"
      workingLabel="Deleting…"
      working={remove.isPending}
      error={error ? docWriteError(error, "delete") : null}
      onConfirm={() => {
        remove.mutate(doc.id, {
          onSuccess: () => {
            onDone?.();
            onOpenChange(false);
          },
        });
      }}
    />
  );
}
