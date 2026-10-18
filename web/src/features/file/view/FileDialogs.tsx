import type { FileContent, FileEntry, SwimlaneDetail, ValidationIssue, Version } from "@/api/generated/model";
import { DeleteDialog } from "../dialogs/DeleteDialog";
import { UploadDialog } from "../dialogs/UploadDialog";
import { DiscardDialog } from "../edit/DiscardDialog";
import type { WriteOutcome } from "../edit/outcome";
import { ConvertEolDialog, SaveDialog } from "../edit/SaveDialog";
import type { EditSession } from "../edit/useEditSession";
import { VersionsDialog } from "../edit/VersionsDialog";
import { RevertDialog } from "../history/RevertDialog";
import { PrDialog } from "../pr/PrDialog";
import { SettledDialogs, type Settled, type WriteSource } from "./SettledDialogs";

// Every dialog of a write, and the answers to them (SettledDialogs). They are one chunk of their own, loaded by FileScreen when the
// browser is idle or when the first dialog is asked for, because none of them is on the way to the file being readable: the file
// view's first load (P10, the Lighthouse page for a file) carries the dialogs' code, and their primitives, only when it is needed.
// FileScreen keeps the state (which dialog, what was settled); this draws it.

export type Dialog =
  | { kind: "save" }
  | { kind: "discard" }
  | { kind: "convert"; to: "lf" | "crlf" }
  | { kind: "upload" }
  | { kind: "delete" }
  | { kind: "pr" }
  | { kind: "versions" }
  | { kind: "revert"; version: Version };

export type FileDialogsProps = {
  /** The file as the screen has it. Without it only the answers (`settled`) can be shown. */
  head: FileContent | undefined;
  lane: string;
  path: string;
  name: string;
  dialog: Dialog | null;
  settled: Settled | null;
  session: EditSession | null;
  entry: FileEntry | null;
  detail: SwimlaneDetail | undefined;
  requiresApproval: boolean;
  /** The hash the person's write was based on. */
  yourBase: string | null;
  canAdopt: boolean;
  canOpenApprovals: boolean;
  onClose: () => void;
  onSettled: (source: WriteSource, outcome: WriteOutcome) => void;
  onIssues: (issues: readonly ValidationIssue[]) => void;
  onTokenExpired: () => void;
  onDiscard: () => void;
  onViewCurrent: () => void;
  onReapply: () => void;
  onCloseSettled: () => void;
};

export default function FileDialogs(props: FileDialogsProps) {
  const {
    head,
    lane,
    path,
    name,
    dialog,
    settled,
    session,
    entry,
    detail,
    requiresApproval,
    yourBase,
    canAdopt,
    canOpenApprovals,
    onClose,
    onSettled,
    onIssues,
    onTokenExpired,
    onDiscard,
    onViewCurrent,
    onReapply,
    onCloseSettled,
  } = props;
  const closeOnDismiss = (open: boolean): void => {
    if (!open) onClose();
  };
  return (
    <>
      {head === undefined ? null : (
        <>
          <SaveDialog
            open={dialog?.kind === "save"}
            onOpenChange={closeOnDismiss}
            lane={lane}
            path={path}
            name={name}
            text={session?.text ?? ""}
            baseHash={session === null ? head.hash : session.base.hash}
            rebasedFrom={session?.rebasedFrom ?? null}
            approval={requiresApproval}
            eol={head.format.eol}
            onSettled={(outcome) => onSettled("save", outcome)}
            onIssues={onIssues}
          />
          <DiscardDialog open={dialog?.kind === "discard"} onOpenChange={closeOnDismiss} name={name} onDiscard={onDiscard} />
          <ConvertEolDialog
            open={dialog?.kind === "convert"}
            onOpenChange={closeOnDismiss}
            lane={lane}
            path={path}
            name={name}
            text={head.text ?? ""}
            baseHash={head.hash}
            to={dialog?.kind === "convert" ? dialog.to : "lf"}
            onSettled={(outcome) => onSettled("convert", outcome)}
          />
          <UploadDialog open={dialog?.kind === "upload"} onOpenChange={closeOnDismiss} lane={lane} path={path} name={name} baseHash={head.hash} onSettled={(outcome) => onSettled("upload", outcome)} />
          <DeleteDialog open={dialog?.kind === "delete"} onOpenChange={closeOnDismiss} lane={lane} path={path} name={name} baseHash={head.hash} onSettled={(outcome) => onSettled("delete", outcome)} />
          <RevertDialog
            open={dialog?.kind === "revert"}
            onOpenChange={closeOnDismiss}
            id={lane}
            path={path}
            version={dialog?.kind === "revert" ? dialog.version : null}
            currentHash={head.hash}
            onSettled={(outcome) => onSettled("revert", outcome)}
          />
          <PrDialog
            open={dialog?.kind === "pr"}
            onOpenChange={closeOnDismiss}
            lane={lane}
            path={path}
            entry={entry}
            tenants={detail?.tenants ?? []}
            hash={head.hash}
            onSettled={(outcome) => onSettled("pr", outcome)}
            onTokenExpired={onTokenExpired}
          />
          {settled?.kind === "conflict" && settled.current !== null ? (
            <VersionsDialog open={dialog?.kind === "versions"} onOpenChange={closeOnDismiss} lane={lane} path={path} name={name} currentHash={settled.current} yours={session?.text ?? ""} />
          ) : null}
        </>
      )}
      <SettledDialogs
        settled={settled}
        onClose={onCloseSettled}
        lane={lane}
        name={name}
        detail={detail}
        yourBase={yourBase}
        canAdopt={canAdopt}
        canOpenApprovals={canOpenApprovals}
        onDiscard={onDiscard}
        onViewCurrent={onViewCurrent}
        onReapply={onReapply}
      />
    </>
  );
}
