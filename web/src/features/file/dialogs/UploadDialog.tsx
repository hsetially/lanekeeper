import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { SafeText } from "@/components/app/SafeText";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { classify, errorInfo, type WriteOutcome } from "../edit/outcome";
import { useWriteDialog } from "../edit/useWriteDialog";
import { fileMutations } from "../mutations";
import { EffectPreview } from "./EffectPreview";

// Upload replaces the file with one from the person's disk (no design page: this follows the Consequence dialogs). Two steps,
// both enforced by the server: the preview (what would change, from the engine, with a token) as soon as a file is chosen, then
// the upload with that token. The page never reads the file: it goes to the server as it is, and the 2 MiB limit is checked
// first only so that a file that cannot be accepted is not sent.

export const MAX_UPLOAD_BYTES = 2 * 1024 * 1024;

export type UploadDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  lane: string;
  path: string;
  name: string;
  /** The hash of the file being replaced, or null when there is none. */
  baseHash: string | null;
  onSettled: (outcome: WriteOutcome) => void;
};

export function UploadDialog({ open, onOpenChange, lane, path, name, baseHash, onSettled }: UploadDialogProps) {
  const client = useQueryClient();
  const inputId = useId();
  const [file, setFile] = useState<File | null>(null);
  const [tooLarge, setTooLarge] = useState(false);
  const preview = useMutation(fileMutations.preview(client));
  const upload = useMutation(fileMutations.upload(client));
  const write = useWriteDialog(upload, (outcome) => {
    onOpenChange(false);
    onSettled(outcome);
  });
  const { reset } = preview;
  const { clear } = write;

  useEffect(() => {
    if (open) return;
    setFile(null);
    setTooLarge(false);
    reset();
    clear();
  }, [open, reset, clear]);

  const choose = (chosen: File | null) => {
    clear();
    reset();
    setFile(chosen);
    setTooLarge(false);
    if (chosen === null) return;
    if (chosen.size > MAX_UPLOAD_BYTES) {
      setTooLarge(true);
      return;
    }
    preview.mutate({
      swimlane: lane,
      body: { action: "upload", path, expected_state: baseHash === null ? "absent" : "hash", ...(baseHash === null ? {} : { expected_hash: baseHash }), file: chosen },
    });
  };

  const previewError = preview.error ? (() => {
    const outcome = classify({ error: preview.error });
    return outcome.kind === "invalid" || outcome.kind === "too_large" || outcome.kind === "failed" ? errorInfo(outcome) : { title: "The preview could not be made", detail: "Nothing was written. Try choosing the file again." };
  })() : null;
  const ready = preview.data !== undefined && file !== null && !preview.isPending;

  return (
    <ConsequenceDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Upload a file to replace ${name}?`}
      consequence={`The file you choose replaces ${name} on the ${lane} NFS share. The version it replaces stays in the history.`}
      actionLabel="Upload file"
      workingLabel="Uploading…"
      size="lg"
      working={write.working}
      canConfirm={ready}
      error={write.error ?? (tooLarge ? { title: "The file is larger than 2 MiB", detail: "Nothing was sent. Choose a smaller file." } : previewError)}
      onConfirm={() => {
        if (file === null || preview.data === undefined) return;
        void write.run({
          swimlane: lane,
          body: { path, expected_state: baseHash === null ? "absent" : "hash", ...(baseHash === null ? {} : { expected_hash: baseHash }), preview_id: preview.data.preview_id, file },
        });
      }}
    >
      <p className="font-mono text-xs text-muted-foreground">
        <SafeText text={path} />
      </p>
      <div className="flex flex-col gap-1">
        <Label htmlFor={inputId}>File to upload</Label>
        <Input
          id={inputId}
          type="file"
          onChange={(event) => {
            choose(event.target.files?.[0] ?? null);
          }}
          className="h-auto py-1"
        />
      </div>
      {preview.isPending ? (
        <div role="status" aria-busy="true" aria-label="Working out what changes" className="flex flex-col gap-2">
          <Skeleton className="h-4 w-3/5" />
          <Skeleton className="h-4 w-4/5" />
        </div>
      ) : null}
      {preview.data ? <EffectPreview preview={preview.data} /> : null}
      {file === null ? <p className="text-sm text-muted-foreground">Choose the file to see what it would change before anything is written.</p> : null}
    </ConsequenceDialog>
  );
}

