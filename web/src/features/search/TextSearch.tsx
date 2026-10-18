import { useEffect, useMemo, useState, type RefObject } from "react";
import { searchText } from "@/api/generated/endpoints/search/search";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchInput } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Skeleton } from "@/components/ui/skeleton";
import { CircleX, Layers, Search } from "@/icons";
import { useQueryState } from "@/lib/query/use-query-state";
import { LanesFilter } from "./LanesFilter";
import { useOneShot } from "./one-shot";
import { relaxations, useRelaxedCounts } from "./relaxations";
import { SuggestionList } from "./Suggestions";
import { groupTextHits, pluralise } from "./results";
import { MAX_GLOB, MAX_PATTERN_BYTES, textKey, textParams, textProblem } from "./text-request";
import { TextResults } from "./TextResults";
import type { Commit } from "./tabs";
import type { LaneInfo } from "./lanes";
import { useDraft } from "./use-draft";
import type { SearchParams } from "./url";

// The Text tab: find text in the config files of every swimlane, literal unless regex is on. The query, the two switches, the
// swimlanes and the glob are in the address, so a search is a link; the screen asks for what the address says and for nothing
// else, and the answer is the server's (the engine's linear-time matching): nothing here builds a pattern or matches a line.

/** The most hits the server sends for one search (`max_total`). */
const MAX_TOTAL = 500;
const PLAIN_PATTERN_ERROR = "The search engine did not accept this search. A regular expression must be valid and not too large or complex, and a path glob must be a valid glob.";

type Props = {
  params: SearchParams;
  commit: Commit;
  lanes: readonly LaneInfo[];
  queryRef: RefObject<HTMLInputElement | null>;
};

function ResultsSkeleton() {
  return (
    <div className="flex flex-col" data-slot="results-skeleton">
      {Array.from({ length: 10 }, (_, i) => (
        <div key={i} className="flex h-7 items-center gap-3 border-b border-border px-3">
          <Skeleton className="h-3 w-10" />
          <Skeleton className={i % 3 === 0 ? "h-3 w-1/3" : i % 3 === 1 ? "h-3 w-1/2" : "h-3 w-2/5"} />
        </div>
      ))}
    </div>
  );
}

export function TextSearch({ params, commit, lanes, queryRef }: Props) {
  const query = useDraft(params.q, (q) => commit({ q }, "replace"));
  const glob = useDraft(params.glob, (g) => commit({ glob: g }, "replace"));
  const laneMap = useMemo(() => new Map(lanes.map((l) => [l.id, l])), [lanes]);

  const problem = textProblem(params);
  const request = problem === undefined ? { key: textKey(params), load: (signal: AbortSignal) => searchText(textParams(params), { signal }) } : null;
  const { source } = useOneShot(request);
  const state = useQueryState(source, { isEmpty: (r) => r.hits.length === 0 });
  const refused = problem === undefined && state.state === "error" && state.error?.status === 400;

  const hits = state.data?.hits;
  const groups = useMemo(() => groupTextHits(hits ?? []), [hits]);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    setCollapsed(new Set());
  }, [state.data]);
  const counts = useMemo(() => new Map(groups.map((g) => [g.swimlane, g.hits])), [groups]);

  const empty = state.state === "empty";
  const candidates = useMemo(() => (empty && lanes.length > 0 ? relaxations(params, lanes.length) : null), [empty, params, lanes.length]);
  const relaxed = useRelaxedCounts(candidates);

  const searched = params.lanes === null ? lanes.length : params.lanes.length;
  const total = lanes.length;
  const invalid = refused || problem?.kind === "too_long" || problem?.kind === "glob_too_long";

  let summary: string;
  if (problem?.kind === "blank") summary = "Type to search";
  else if (invalid) summary = "Pattern not searched";
  else if (problem?.kind === "no_lanes") summary = "No swimlanes selected";
  else if (state.state === "loading") summary = "Searching…";
  else if (state.state === "error") summary = "Search failed";
  else if (state.data?.truncated === true) summary = `${hits?.length ?? 0}+ matches`;
  else summary = `${pluralise(hits?.length ?? 0, "match", "matches")} in ${pluralise(groups.reduce((n, g) => n + g.files.length, 0), "file", "files")} · ${pluralise(groups.length, "swimlane", "swimlanes")}`;
  const mode = `${params.regex ? "Regex" : "Literal"} · ${params.caseSensitive ? "case sensitive" : "ignoring case"} · searching ${searched} of ${total} swimlanes${params.glob.trim() === "" ? "" : ` · ${params.glob.trim()}`}`;

  let body;
  if (problem?.kind === "blank") {
    body = (
      <EmptyState icon={Search} title={`Search ${total} swimlanes`} className="pt-10">
        Type a query to find it in every config file. Matching is literal unless you turn on regex.
      </EmptyState>
    );
  } else if (problem?.kind === "no_lanes") {
    body = (
      <EmptyState icon={Layers} title="No swimlanes selected" className="pt-10">
        Pick at least one swimlane to search, or select all of them.
      </EmptyState>
    );
  } else if (invalid) {
    body = (
      <EmptyState icon={CircleX} title="Pattern not searched" className="pt-10">
        {params.regex && problem === undefined
          ? `Results stay empty until the pattern is valid. Turn regex off to search for “${visibleText(params.q)}” as plain text.`
          : "Results stay empty until the search is valid."}
      </EmptyState>
    );
  } else {
    body = (
      <QueryBoundaryView
        state={state}
        noun="search results"
        source="the search index"
        loading={<ResultsSkeleton />}
        staleBanner={false}
        className="flex min-h-0 flex-1 flex-col"
        empty={
          <div className="flex flex-col items-center gap-4 px-6 py-10">
            <EmptyState icon={Search} title={`No matches for “${visibleText(params.q)}”`} className="p-0">
              <SafeText text={`Searched ${searched} of ${total} swimlanes. ${params.regex ? "Regex" : "Literal text"}, ${params.caseSensitive ? "case sensitive" : "ignoring case"}${params.glob.trim() === "" ? "" : `, paths matching ${params.glob.trim()}`}.`} />
            </EmptyState>
            <SuggestionList
              items={(candidates ?? [])
                .filter((item) => relaxed.has(item.id))
                .map((item) => {
                  const count = relaxed.get(item.id);
                  return {
                    id: item.id,
                    label: item.label,
                    ...(count ? { note: `${count.capped ? `${count.matches}+ matches` : pluralise(count.matches, "match", "matches")} in ${pluralise(count.files, "file", "files")}` } : {}),
                    action: "Try this",
                    onApply: () => commit(item.patch, "push"),
                  };
                })}
            />
            <p className="max-w-[600px] text-center text-xs text-muted-foreground">Search is literal by default, so “.” and “*” match themselves. Turn on regex for patterns.</p>
          </div>
        }
      >
        {(result) => (
          <>
            {result.truncated ? <TruncationBanner shown={result.hits.length} noun="matches" /> : null}
            <TextResults groups={groups} lanes={laneMap} collapsed={collapsed} onCollapsedChange={setCollapsed} />
            {result.truncated ? (
              <p role="status" className="m-0 flex h-9 shrink-0 items-center gap-2 border-t border-warning-border bg-warning-bg px-3 text-warning-fg">
                {/* A cap on the whole answer ends the list; a cap on one file leaves out lines of that file only. */}
                {result.hits.length >= MAX_TOTAL ? (
                  <>
                    <strong className="font-semibold">Results stop here.</strong>
                    <span>{`${result.hits.length.toLocaleString("en-US")} matches are listed. Later swimlanes and files are not shown.`}</span>
                  </>
                ) : (
                  <>
                    <strong className="font-semibold">Some files have more matching lines than are listed.</strong>
                    <span>Narrow the query to see them.</span>
                  </>
                )}
              </p>
            ) : null}
          </>
        )}
      </QueryBoundaryView>
    );
  }

  const ready = state.state === "ready" && groups.length > 0;
  return (
    <>
      <div className="flex shrink-0 flex-col gap-1.5 border-b border-border bg-muted p-2">
        <div className="flex items-center gap-2">
          <SearchInput
            ref={queryRef}
            aria-label="Query"
            placeholder="Find text in config files"
            kbd="/"
            value={query.value}
            onChange={(event) => query.set(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") query.flush();
            }}
            aria-invalid={invalid || undefined}
            {...(invalid ? { "aria-describedby": "q-error" } : {})}
            className="font-mono text-xs"
            {...(invalid ? { boxClassName: "border-destructive" } : {})}
            toggles={[
              { id: "case", label: "Match case", short: "Aa", pressed: params.caseSensitive, onPressedChange: (caseSensitive) => commit({ caseSensitive }, "push") },
              { id: "regex", label: "Use regular expression", short: ".*", pressed: params.regex, onPressedChange: (regex) => commit({ regex }, "push") },
            ]}
          />
          <LanesFilter lanes={lanes} selected={params.lanes} onChange={(next) => commit({ lanes: next }, "push")} counts={ready ? counts : undefined} />
          <Input
            aria-label="Path glob"
            placeholder="Path glob, e.g. **/*-sit7.yml"
            maxLength={MAX_GLOB}
            value={glob.value}
            onChange={(event) => glob.set(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") glob.flush();
            }}
            className="w-[264px] shrink-0 font-mono text-xs"
          />
        </div>
        {invalid ? (
          <div id="q-error" role="alert" className="flex flex-wrap items-center gap-x-2 gap-y-1 text-danger-fg">
            <CircleX aria-hidden="true" className="size-3.5 shrink-0" />
            {problem?.kind === "too_long" ? (
              <span>
                <strong className="font-semibold">Pattern is too long.</strong> {`The pattern is longer than ${MAX_PATTERN_BYTES} bytes (it is ${problem.bytes}). Shorten it.`}
              </span>
            ) : problem?.kind === "glob_too_long" ? (
              <span>
                <strong className="font-semibold">Path glob is too long.</strong> {`A glob can be at most ${MAX_GLOB} characters.`}
              </span>
            ) : (
              <>
                <strong className="font-semibold">{params.regex ? "Invalid regular expression." : "Invalid search."}</strong>
                <span>{PLAIN_PATTERN_ERROR}</span>
                {state.error?.requestId ? <RequestId id={state.error.requestId} /> : null}
                {params.regex ? (
                  <Button type="button" variant="secondary" size="sm" className="ml-auto" onClick={() => commit({ regex: false }, "push")}>
                    Search as plain text
                  </Button>
                ) : null}
              </>
            )}
          </div>
        ) : null}
      </div>
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-border px-3 text-xs text-muted-foreground">
        <strong aria-live="polite" className="font-semibold tabular-nums text-foreground">
          {summary}
        </strong>
        <span>
          <SafeText text={mode} />
        </span>
        {ready ? (
          <span className="ml-auto inline-flex gap-1">
            <Button type="button" variant="ghost" size="sm" className="text-xs font-normal" onClick={() => setCollapsed(new Set(groups.map((g) => g.id)))}>
              Collapse all
            </Button>
            <Button type="button" variant="ghost" size="sm" className="text-xs font-normal" onClick={() => setCollapsed(new Set())}>
              Expand all
            </Button>
          </span>
        ) : null}
      </div>
      {body}
      {ready ? (
        <div className="flex h-8 shrink-0 items-center gap-4 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
          <span>
            <Kbd>↑</Kbd> <Kbd>↓</Kbd> move
          </span>
          <span>
            <Kbd>↵</Kbd> open file at line
          </span>
          <span className="ml-auto">Every result opens its file at the matching line.</span>
        </div>
      ) : null}
    </>
  );
}
