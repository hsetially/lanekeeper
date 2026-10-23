import { GatedButton } from "@/components/app/GatedButton";
import { Button } from "@/components/ui/button";
import { BookOpen, GitPullRequest, History, Pencil, Server, Trash2, Upload } from "@/icons";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import type { FileGates } from "./gates";

// The actions on the right of the file's head. A write button is disabled with its reason, never hidden for a reason a
// person can fix (a role, an agent that is offline); a file whose content is withheld has no write action at all, as the
// design draws it (File, "denied"). History and Docs for this file are toggles for the panels on the right. "As served" is a link to
// the page of the text the config-server serves (`/swimlanes/:id/served`), opened with this file and its tenant filled in; a file
// whose content is withheld has none, because that page would show the served text of a file the hub does not show.

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
  /** The swimlane, the file and the file's tenant (null: it belongs to none) for the As served link. Without it there is no link. */
  asServed?: { id: string; file: string; tenant: string | null } | null;
};

export function FileActions({ gates, editing, withheld, historyOpen, docs, onEdit, onPr, onUpload, onDelete, onHistory, onDocs, asServed = null }: FileActionsProps) {
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
      {asServed === null || withheld ? null : (
        <Button asChild variant="secondary">
          <AppLink route="swimlane-served" params={{ id: asServed.id }} search={{ file: asServed.file, ...(asServed.tenant === null ? {} : { tenant: asServed.tenant }) }}>
            <Server aria-hidden="true" />
            As served
          </AppLink>
        </Button>
      )}
    </>
  );
}
