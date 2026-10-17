// TEST PAGE (perf build only): the diff of a 6,000-line file of P10. The hunks come from the generated `compareFile` (with
// an ETag the app's fetch layer revalidates), and DiffView draws them. It marks when the diff is on screen with the keyboard
// and when the first `n` moved it, which is what the spec times, and it warms the editor for an Editor and above, as the
// file view will.
import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { DiffView, type DiffViewHandle } from "@/components/app/diff/DiffView";
import { diffStats } from "@/components/app/diff/rows";
import { useEditorWarmup } from "@/editor/warm";
import { compareFileQuery } from "@/features/compare/queries";
import { QUERY_STALE_MS } from "@/lib/query/client";
import { resetLab, setLab } from "./lab";

const PARAMS = { left: "nfs:swimlane-01", right: "git:base@main", file: "services/payments/application.yml" } as const;

export function PerfDiff() {
  // The page keeps the client's 15 s freshness so that what the spec measures (a warm, a revalidated and a cold diff) is what it was.
  const query = useQuery({ ...compareFileQuery(PARAMS.left, PARAMS.right, PARAMS.file, null), staleTime: QUERY_STALE_MS });
  const [mode, setMode] = useState<"split" | "unified">("split");
  const ref = useRef<DiffViewHandle>(null);
  const hunks = query.data?.hunks;
  const interactive = hunks !== undefined;

  useEditorWarmup(interactive);

  useEffect(() => {
    resetLab();
    return resetLab;
  }, []);
  // The diff is up once it has been committed with its hunks: put the keyboard on it (a file view does the same) and say so.
  useEffect(() => {
    if (!interactive) return;
    ref.current?.focus();
    setLab({ phase: "ready", diffReady: performance.now() });
  }, [interactive]);

  const stats = hunks ? diffStats(hunks) : null;
  return (
    <main className="flex h-screen flex-col">
      <h1 className="p-3 text-lg font-semibold">Diff</h1>
      <div className="flex items-center gap-3 px-3 pb-2 text-sm text-muted-foreground">
        <Link to="/testpages/perf/nav/a" className="underline">
          Back to the pages
        </Link>
        {stats ? (
          <span>
            +{stats.added} -{stats.removed}
          </span>
        ) : null}
        <button type="button" className="rounded-sm border border-border px-2" onClick={() => setMode((m) => (m === "split" ? "unified" : "split"))}>
          {mode === "split" ? "Unified" : "Split"}
        </button>
      </div>
      {hunks ? (
        <DiffView
          ref={ref}
          data-testid="diff"
          label="Diff of services/payments/application.yml"
          hunks={hunks}
          mode={mode}
          className="min-h-0 flex-1"
          onActiveChange={() => {
            if (window.__perf?.firstN === undefined) setLab({ firstN: performance.now() });
          }}
        />
      ) : (
        <p className="p-3 text-sm text-muted-foreground" aria-busy="true">
          {query.isError ? "The diff could not be loaded." : "Loading the diff"}
        </p>
      )}
    </main>
  );
}
