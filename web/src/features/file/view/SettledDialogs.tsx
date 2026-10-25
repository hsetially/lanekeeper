import type { SwimlaneDetail } from "@/api/generated/model";
import { ConflictDialog, LockedDialog, SentDialog } from "../edit/OutcomeDialogs";
import { unknownFileCount } from "../model";

// The dialogs of the three answers to a write that is not a plain success, for the file screen: sent for approval, the file
// changed first (409) and locked (423). Every write of the screen (save, convert, revert, upload, delete, raise PR) can end in
// one of them, so the screen keeps what the last write ended in and this shows it. Nothing here is a guess: it is what the
// server answered, in the words of the States index.

export type WriteSource = "save" | "convert" | "revert" | "upload" | "delete" | "pr";

export type Settled =
  | { kind: "sent"; proposal: number; source: WriteSource }
  | { kind: "conflict"; current: string | null; requestId: string | undefined; source: WriteSource }
  | { kind: "locked"; until: number | null; requestId: string | undefined; source: WriteSource };

const WHAT: Record<WriteSource, string | undefined> = { save: undefined, convert: "conversion", revert: "revert", upload: "upload", delete: "delete", pr: "pull request" };
const ACTION: Record<WriteSource, string> = { save: "Save", convert: "Convert line endings", revert: "Revert", upload: "Upload", delete: "Delete", pr: "Raise PR" };

export type SettledDialogsProps = {
  settled: Settled | null;
  onClose: () => void;
  lane: string;
  name: string;
  detail: SwimlaneDetail | undefined;
  /** The hash the person's write was based on. */
  yourBase: string | null;
  canAdopt: boolean;
  canOpenApprovals: boolean;
  /** The three ways forward after an edit's 409. */
  onDiscard: () => void;
  onViewCurrent: () => void;
  onReapply: () => void;
};

export function SettledDialogs({ settled, onClose, lane, name, detail, yourBase, canAdopt, canOpenApprovals, onDiscard, onViewCurrent, onReapply }: SettledDialogsProps) {
  const closeWhenShut = (open: boolean): void => {
    if (!open) onClose();
  };
  const conflict = settled?.kind === "conflict" ? settled : null;
  const locked = settled?.kind === "locked" ? settled : null;
  const sent = settled?.kind === "sent" ? settled : null;
  const what = conflict === null ? undefined : WHAT[conflict.source];
  return (
    <>
      <ConflictDialog
        open={conflict !== null}
        onOpenChange={closeWhenShut}
        file={name}
        lane={lane}
        yourBase={yourBase}
        current={conflict?.current ?? null}
        currentNote={null}
        requestId={conflict?.requestId}
        what={what}
        {...(what === undefined ? { onDiscard, onViewCurrent, onReapply } : {})}
      />
      <LockedDialog
        open={locked !== null}
        onOpenChange={closeWhenShut}
        action={locked === null ? "" : `${ACTION[locked.source]} on ${lane}`}
        lane={lane}
        baseline={detail?.baseline?.state}
        unknownFiles={unknownFileCount(detail?.drift_counts)}
        until={locked?.until ?? null}
        canAdopt={canAdopt}
        requestId={locked?.requestId}
      />
      <SentDialog open={sent !== null} onOpenChange={closeWhenShut} file={name} lane={lane} proposal={sent?.proposal ?? 0} canOpenApprovals={canOpenApprovals} />
    </>
  );
}
