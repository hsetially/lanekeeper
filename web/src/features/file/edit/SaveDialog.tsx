import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type { EolStyle, ValidationIssue } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { HashChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { eolLabel } from "../model";
import { fileMutations } from "../mutations";
import type { WriteOutcome } from "./outcome";
import { useWriteDialog } from "./useWriteDialog";

// The review before a save (the keymap: "opens the consequence dialog; never saves silently"), and the same for converting a
// file's line endings. The consequence comes first, the primary button names the action, initial focus is on Cancel, and the
// write carries the hash the edit was based on: a file that changed under the person is a conflict and nothing is written.
// The answer is the server's. Applied, sent for approval, 409 and 423 go to the page (its dialogs and banner); a refused
// content closes this and is marked in the editor; anything else stays here, with the request id, and the text intact.

type Common = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  lane: string;
  path: string;
  name: string;
  /** The text to write. */
  text: string;
  /** The hash the text is based on, or null when the file was not there. */
  baseHash: string | null;
  onSettled: (outcome: WriteOutcome) => void;
};

export type SaveDialogProps = Common & {
  /** The hash this edit was based on before the person chose to re-apply on top of a newer version. */
  rebasedFrom: string | null;
  approval: boolean;
  eol: EolStyle;
  /** The server refused the content: where and why, as lines and rule names. */
  onIssues: (issues: readonly ValidationIssue[]) => void;
};

const expectedOf = (hash: string | null) => (hash === null ? ({ state: "absent" } as const) : ({ state: "hash", hash } as const));

export function SaveDialog({ open, onOpenChange, lane, path, name, text, baseHash, rebasedFrom, approval, eol, onSettled, onIssues }: SaveDialogProps) {
  const client = useQueryClient();
  const mutation = useMutation(fileMutations.edit(client));
  const write = useWriteDialog(mutation, (outcome) => {
    onOpenChange(false);
    onSettled(outcome);
  });
  const { issues, clear } = write;
  useEffect(() => {
    if (issues.length === 0) return;
    onIssues(issues);
    clear();
    onOpenChange(false);
  }, [issues, clear, onIssues, onOpenChange]);
  useEffect(() => {
    if (!open) clear();
  }, [open, clear]);

  return (
    <ConsequenceDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Save ${name} to ${lane}?`}
      consequence={approval ? `This change needs an Admin to approve it before it is written to NFS. Nothing changes on ${lane} until then.` : `Saving writes straight to the ${lane} NFS share. Git changes only through a PR.`}
      actionLabel={approval ? "Submit for approval" : `Save to ${lane} NFS`}
      workingLabel={approval ? "Submitting…" : "Saving…"}
      working={write.working}
      error={write.error}
      onConfirm={() => {
        void write.run({ swimlane: lane, body: { path, expected: expectedOf(baseHash), content: text } });
      }}
    >
      <p className="font-mono text-xs text-muted-foreground">
        <SafeText text={path} />
      </p>
      <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
        <dt className="text-muted-foreground">Based on</dt>
        <dd className="m-0 flex flex-wrap items-center gap-1.5">{baseHash === null ? "No file: a new one is created" : <HashChip hash={baseHash} />}</dd>
        <dt className="text-muted-foreground">Format</dt>
        <dd className="m-0">The file keeps its line endings ({eolLabel(eol)}), encoding and BOM.</dd>
        {rebasedFrom === null ? null : (
          <>
            <dt className="text-muted-foreground">Note</dt>
            <dd className="m-0">This replaces the version that is on NFS now, which someone else saved, with yours.</dd>
          </>
        )}
      </dl>
    </ConsequenceDialog>
  );
}

export type ConvertEolDialogProps = Common & { to: "lf" | "crlf" };

/** Convert a file's line endings: the same text, written with the other ending (`change_eol`). */
export function ConvertEolDialog({ open, onOpenChange, lane, path, name, text, baseHash, to, onSettled }: ConvertEolDialogProps) {
  const client = useQueryClient();
  const mutation = useMutation(fileMutations.edit(client));
  const write = useWriteDialog(mutation, (outcome) => {
    onOpenChange(false);
    onSettled(outcome);
  });
  const { clear } = write;
  useEffect(() => {
    if (!open) clear();
  }, [open, clear]);
  const ending = eolLabel(to);
  return (
    <ConsequenceDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Convert ${name} to ${ending}?`}
      consequence={`The text stays the same; only the line endings change. This writes the file on the ${lane} NFS share.`}
      actionLabel={`Convert to ${ending}`}
      workingLabel="Converting…"
      working={write.working}
      error={write.error}
      onConfirm={() => {
        void write.run({ swimlane: lane, body: { path, expected: expectedOf(baseHash), content: text, change_eol: to } });
      }}
    >
      <p className="font-mono text-xs text-muted-foreground">
        <SafeText text={path} />
      </p>
      <p className="text-sm">The file stays NFS ahead or becomes in sync, depending on Git.</p>
    </ConsequenceDialog>
  );
}
