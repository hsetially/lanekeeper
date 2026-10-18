import { GatedButton } from "@/components/app/GatedButton";
import { Button } from "@/components/ui/button";
import { BookOpen, GitPullRequest, History, Pencil, Trash2, Upload } from "@/icons";
import { cn } from "@/lib/utils";
import type { FileGates } from "./gates";

// The actions on the right of the file's head. A write button is disabled with its reason, never hidden for a reason a
// person can fix (a role, an agent that is offline); a file whose content is withheld has no write action at all, as the
// design draws it (File, "denied"). History and Docs for this file are toggles for the panels on the right.

export type FileActionsProps = {
  gates: FileGates;
  editing: boolean;
  withheld: boolean;
  historyOpen: boolean;
  /** "none": the file has no docs, so there is no button. */
  docs: "none" | "available" | "open";
  onEdit: () => void;
  onPr: () => void;
  onUpload: () => void;
  onDelete: () => void;
  onHistory: () => void;
  onDocs: () => void;
};

export function FileActions({ gates, editing, withheld, historyOpen, docs, onEdit, onPr, onUpload, onDelete, onHistory, onDocs }: FileActionsProps) {
  return (
    <>
      {withheld ? null : (
        <>
          <GatedButton variant="secondary" gate={editing ? { allowed: false, reason: "You are editing this file." } : gates.edit} onClick={onEdit}>
            <Pencil aria-hidden="true" />
            Edit
          </GatedButton>
          <GatedButton variant="secondary" gate={gates.pr} onClick={onPr}>
            <GitPullRequest aria-hidden="true" />
            Raise PR
          </GatedButton>
          <GatedButton variant="secondary" gate={gates.upload} onClick={onUpload}>
            <Upload aria-hidden="true" />
            Upload
          </GatedButton>
          <GatedButton variant="ghost" gate={gates.delete} onClick={onDelete} className="border-danger-border text-danger-fg hover:bg-danger-bg">
            <Trash2 aria-hidden="true" />
            Delete
          </GatedButton>
        </>
      )}
      <Button variant="secondary" aria-pressed={historyOpen} onClick={onHistory} className={cn(historyOpen && "border-ring bg-accent")}>
        <History aria-hidden="true" />
        History
      </Button>
      {docs === "none" || withheld ? null : (
        <Button variant="secondary" aria-pressed={docs === "open"} onClick={onDocs} className={cn(docs === "open" && "border-ring bg-accent")}>
          <BookOpen aria-hidden="true" />
          Docs for this file
        </Button>
      )}
    </>
  );
}
