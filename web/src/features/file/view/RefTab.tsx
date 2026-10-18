import { useQuery } from "@tanstack/react-query";
import type { FileEntry, SwimlaneDetail } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { useQueryState } from "@/lib/query/use-query-state";
import { fileBaselineQuery, fileRefQuery } from "../queries";
import { ContentView } from "./ContentView";

// The Git and Baseline tabs: the same file as another place has it. Git is the tenant's branch head for a tenant file and the
// swimlane's base version for a base file; the baseline is the swimlane's own, adopted copy. A file that is not there is a
// state of its own ("not in Git", "not in the baseline"), not an error.

export type RefTabProps = {
  source: "git" | "baseline";
  lane: string;
  path: string;
  name: string;
  /** The file's listing entry, or null while it is not known (the Git side depends on its tenant). */
  entry: FileEntry | null;
  entryKnown: boolean;
  detail: SwimlaneDetail | undefined;
  line: number | null;
  onCompare: () => void;
};

const LOADING = { state: "loading", data: undefined, error: null, retryable: false, refreshing: false, stale: null, refetch: () => undefined } as const;

export function RefTab({ source, lane, path, name, entry, entryKnown, detail, line, onCompare }: RefTabProps) {
  const tenant = entry?.tenant ?? null;
  const commit = detail?.base_version?.commit ?? null;
  const ref = tenant ?? commit;
  const gitReady = entryKnown && detail !== undefined;
  const git = useQuery({ ...fileRefQuery(lane, path, tenant !== null ? "tenant" : "base", ref ?? ""), enabled: source === "git" && gitReady && ref !== null });
  const baseline = useQuery({ ...fileBaselineQuery(lane, path), enabled: source === "baseline" });
  const query = source === "git" ? git : baseline;
  const state = useQueryState(query);
  const where = source === "git" ? "Git" : "the baseline";

  if (source === "git" && gitReady && ref === null) {
    return <EmptyState title="No Git version to show">{`${lane} has no base version, so there is no Git copy of a base file.`}</EmptyState>;
  }
  if (state.state === "error" && state.error?.status === 404) {
    return source === "git" ? (
      <EmptyState title={`${name} is not in Git`}>The file exists on NFS only. Raise a pull request to add it to Git.</EmptyState>
    ) : (
      <EmptyState title={`${name} is not in the baseline`}>The baseline was adopted without this file, so there is nothing to compare it with.</EmptyState>
    );
  }
  return (
    <QueryBoundaryView state={source === "git" && !gitReady ? LOADING : state} noun={source === "git" ? "the Git version" : "the baseline"} className="flex min-h-0 flex-1 flex-col">
      {(content) => <ContentView content={content} name={name} where={where} highlightLine={line} entry={entry} onCompareWithGit={onCompare} />}
    </QueryBoundaryView>
  );
}
