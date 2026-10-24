import { EmptyState } from "@/components/app/EmptyState";
import { Button } from "@/components/ui/button";
import { AppLink } from "@/routes/AppLink";
import type { PageProps } from "@/routes/props";
import { parseDiffParam } from "./diff/refs";
import { isPlausibleNfsPath, parseLine, parsePanel, parseTab } from "./model";
import { FileScreen } from "./view/FileScreen";

// The file view: one file of one swimlane, on NFS, in Git and in the baseline, and the difference between them. The address
// holds which file (`path`) and what is shown (`tab`, `panel`, `diff`, `line`). Every value in it is untrusted input: the
// tab, the panel, the side and the line are taken only if they are ones the screen knows, and a path that cannot be a file
// path is not sent to the server at all.

export default function FilePage({ params, search }: PageProps<"files">) {
  const path = search.path;
  if (path === undefined || path === "") {
    return (
      <EmptyState
        as="h1"
        title="Choose a file"
        action={
          <Button asChild variant="secondary">
            <AppLink route="swimlane" params={{ id: params.id }}>
              Open the swimlane
            </AppLink>
          </Button>
        }
        className="pt-16"
      >
        This address names a swimlane but no file. Pick one from the swimlane's files.
      </EmptyState>
    );
  }
  if (!isPlausibleNfsPath(path)) {
    return (
      <EmptyState as="h1" title="That is not a file path" className="pt-16">
        The address holds a path that cannot be a file in a swimlane, so nothing was asked of the server.
      </EmptyState>
    );
  }
  return <FileScreen key={`${params.id}:${path}`} id={params.id} path={path} tab={parseTab(search.tab)} panel={parsePanel(search.panel)} side={parseDiffParam(search.diff)} line={parseLine(search.line)} />;
}
