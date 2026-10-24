import { useQuery } from "@tanstack/react-query";
import { QueryBoundary } from "@/components/app/QueryBoundary";
import { CodeView } from "@/components/app/virtual/CodeView";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ContentView } from "../view/ContentView";
import { fileContentQuery } from "../queries";

// The two texts after a 409: what is on NFS now (read by its hash) beside what the person has. The contract has no comparison
// of two stored versions, so this shows them as they are and computes nothing (rule 7); the person decides whether to re-apply.

export type VersionsDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  lane: string;
  path: string;
  name: string;
  /** The hash on NFS now. */
  currentHash: string;
  /** What the person has in the editor. */
  yours: string;
};

function Current({ lane, path, name, hash }: { lane: string; path: string; name: string; hash: string }) {
  const query = useQuery(fileContentQuery(lane, path, hash));
  return (
    <QueryBoundary query={query} noun="the version on NFS now" className="min-h-0 flex-1">
      {(content) => <ContentView content={content} name={name} where="NFS" highlightLine={null} entry={null} onCompareWithGit={null} />}
    </QueryBoundary>
  );
}

export function VersionsDialog({ open, onOpenChange, lane, path, name, currentHash, yours }: VersionsDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="xl" className="max-w-[1100px]">
        <DialogHeader>
          <DialogTitle>Compare versions</DialogTitle>
          <DialogDescription>Lanekeeper does not merge two versions. Read what is on NFS now beside your own, then re-apply yours on top, or discard it.</DialogDescription>
        </DialogHeader>
        <div className="grid grid-cols-2 gap-3">
          <section aria-label="On NFS now" className="flex h-[50vh] min-w-0 flex-col overflow-hidden rounded-md border border-border">
            <h3 className="m-0 flex-none border-b border-border bg-muted px-3 py-1.5 text-sm font-semibold">On NFS now</h3>
            <Current lane={lane} path={path} name={`${name} (on NFS now)`} hash={currentHash} />
          </section>
          <section aria-label="Yours" className="flex h-[50vh] min-w-0 flex-col overflow-hidden rounded-md border border-border">
            <h3 className="m-0 flex-none border-b border-border bg-muted px-3 py-1.5 text-sm font-semibold">Yours</h3>
            <div className="min-h-0 flex-1 bg-card pt-1">
              <CodeView label={`${name} (yours)`} text={yours} className="h-full" />
            </div>
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}
