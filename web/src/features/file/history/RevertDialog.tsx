import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type { Version } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { SafeText } from "@/components/app/SafeText";
import { describeAttribution } from "@/components/attribution";
import { useActorNames } from "@/features/attribution";
import { formatTime } from "@/lib/time";
import { fileMutations } from "../mutations";
import type { WriteOutcome } from "../edit/outcome";
import { useWriteDialog } from "../edit/useWriteDialog";
import { sourceLabel } from "./attribution";

// Revert to a version (History, 1k): the consequence first, the version named, and the primary button says which one. A revert
// is a write like any other: it carries the hash that is on NFS now, so a file that changed under the person is a conflict and
// nothing is written; it may be sent for approval, and it may be locked. Those three go to the screen (its dialogs); a refusal
// or a failure stays here, inline, with the request id.

export type RevertDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  id: string;
  path: string;
  version: Version | null;
  /** The hash on NFS now, or null when the file is not there. */
  currentHash: string | null;
  onSettled: (outcome: WriteOutcome) => void;
};

const short = (hash: string): string => Array.from(hash).slice(0, 7).join("");

export function RevertDialog({ open, onOpenChange, id, path, version, currentHash, onSettled }: RevertDialogProps) {
  const client = useQueryClient();
  const { you, nameOf } = useActorNames();
  const mutation = useMutation(fileMutations.revert(client));
  const write = useWriteDialog(mutation, (outcome) => {
    onOpenChange(false);
    onSettled(outcome);
  });
  const { clear } = write;
  useEffect(() => {
    if (!open) clear();
  }, [open, clear]);

  if (version === null) return null;
  const who = describeAttribution(version.attribution, { you, nameOf }).who;
  const name = short(version.hash);
  return (
    <ConsequenceDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Revert to ${name}?`}
      consequence="This replaces the file on NFS with the content of this version. The version you replace stays in the history."
      actionLabel={`Revert to ${name}`}
      workingLabel="Reverting…"
      working={write.working}
      error={write.error}
      onConfirm={() => {
        void write.run({
          swimlane: id,
          body: { path, expected: currentHash === null ? { state: "absent" } : { state: "hash", hash: currentHash }, to: version.hash },
        });
      }}
    >
      <p className="font-mono text-xs text-muted-foreground">
        <SafeText text={path} />
      </p>
      <dl className="grid grid-cols-[72px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
        <dt className="text-muted-foreground">Version</dt>
        <dd className="m-0">
          {sourceLabel(version.source)} by <SafeText text={who.label} />, {formatTime(version.observed_at)}
        </dd>
        <dt className="text-muted-foreground">Git</dt>
        <dd className="m-0">The file stays NFS ahead or becomes in sync, depending on Git. Nothing is deleted from history.</dd>
      </dl>
    </ConsequenceDialog>
  );
}
