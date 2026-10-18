import { useMemo, useState, type MouseEvent, type RefObject } from "react";
import { searchDocs } from "@/api/generated/endpoints/docs/docs";
import type { DocHit, DocSummary } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { SearchInput } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { docSource } from "@/features/docs/doc-path";
import { docListQuery } from "@/features/docs/queries";
import { UploadDocDialog } from "@/features/docs/UploadDocDialog";
import { formatBytes } from "@/features/docs/upload";
import { BookOpen, ChevronRight, GitBranch, Upload, User } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatRelative } from "@/lib/time";
import { AppLink, isPlainClick, useNavigateTo } from "@/routes/AppLink";
import { useOneShot } from "./one-shot";
import { docHref, pluralise, snippetRuns } from "./results";
import { SuggestionList, type Suggestion } from "./Suggestions";
import type { Commit } from "./tabs";
import type { SearchParams } from "./url";
import { useDraft } from "./use-draft";

// The Docs tab (frames 1j, 1n and 1o to 1t): the docs, and a search over their sections. With no query it lists the docs, so
// there is somewhere to start; with one it asks the server for the best matches (hybrid ranking, at most 50, no cursor) and each
// result opens the reader at the heading the server named. Upload is here, for Editors and above.

/** `searchDocs` has `q` at most 512 characters (the address keeps at most 512 of a value) and `limit` at most 50. */
const MAX_QUERY = 512;
const BEST = 50;
const PAGE_SIZE = 100;

type Source = "all" | "git" | "upload";
const SOURCE_LABEL: Record<Exclude<Source, "all">, string> = { git: "Git", upload: "Uploaded" };

// The source filter is drawn as the design's bordered segments: 26 px, a divider between them, the chosen one on the secondary colour.
const SEGMENT = "h-[26px] rounded-none border-0 border-l border-input px-2.5 font-normal text-foreground first:border-l-0 data-[state=on]:border-input data-[state=on]:bg-secondary data-[state=on]:font-medium hover:bg-secondary-hover";

type Props = { params: SearchParams; commit: Commit; queryRef: RefObject<HTMLInputElement | null> };

export function SourceChip({ source }: { source: "git" | "upload" | undefined }) {
  if (source === undefined) return null;
  const Icon = source === "git" ? GitBranch : User;
  return (
    <span className="inline-flex h-5 shrink-0 items-center gap-1 rounded-sm border border-border px-1.5 text-xs text-muted-foreground">
      <Icon aria-hidden="true" className="size-3" />
      {SOURCE_LABEL[source]}
    </span>
  );
}

function Snippet({ text, terms }: { text: string; terms: readonly string[] }) {
  return (
    <span className="max-w-[900px] text-[13px] leading-5 text-pretty">
      {snippetRuns(text, terms).map((run, i) => {
        const inner = run.segments.map((segment, j) =>
          segment.mark ? (
            <mark key={j} className="rounded-[2px] bg-selection px-0 font-semibold text-foreground shadow-[inset_0_0_0_1px_var(--info-border)]">
              <SafeText text={segment.text} />
            </mark>
          ) : (
            <SafeText key={j} text={segment.text} />
          ),
        );
        return run.code ? (
          <code key={i} className="rounded-[3px] border border-border bg-muted px-[3px] font-mono text-xs">
            {inner}
          </code>
        ) : (
          <span key={i}>{inner}</span>
        );
      })}
    </span>
  );
}

function Section({ hit, terms }: { hit: DocHit; terms: readonly string[] }) {
  const goTo = useNavigateTo();
  const href = docHref(hit.path, hit.anchor);
  const file = hit.path.slice(hit.path.lastIndexOf("/") + 1);
  const trail = hit.heading_path.length === 0 ? [hit.title] : hit.heading_path;
  const body = (
    <>
      <span className="flex items-center gap-2">
        <BookOpen aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        <strong className="text-base leading-5 font-semibold text-link">
          <SafeText text={hit.title} />
        </strong>
        <span className="ml-auto">
          <SourceChip source={docSource(hit.path)} />
        </span>
      </span>
      <span className="flex flex-wrap items-center gap-1 text-xs leading-[18px] font-medium text-muted-foreground">
        {trail.map((step, i) => (
          <span key={i} className="inline-flex items-center gap-1">
            <span className={i === trail.length - 1 ? "text-foreground" : undefined}>
              <SafeText text={step} />
            </span>
            {i < trail.length - 1 ? <ChevronRight aria-hidden="true" className="size-3" /> : null}
          </span>
        ))}
      </span>
      <Snippet text={hit.snippet} terms={terms} />
      <span className="font-mono text-xs leading-[18px] text-muted-foreground">
        <SafeText text={hit.anchor ? `${file}#${hit.anchor}` : file} />
      </span>
    </>
  );
  const className = "flex flex-col gap-1 border-b border-border px-3 py-2.5 text-foreground no-underline hover:bg-accent hover:text-foreground";
  return (
    <li>
      {href === undefined ? (
        <div className={className}>{body}</div>
      ) : (
        <a
          href={href}
          rel="noopener noreferrer"
          aria-label={`${hit.title}: ${hit.heading_path.join(" > ")}`}
          onClick={(event: MouseEvent<HTMLAnchorElement>) => {
            if (!isPlainClick(event)) return;
            event.preventDefault();
            goTo(href);
          }}
          className={className}
        >
          {body}
        </a>
      )}
    </li>
  );
}

export function DocsSearch({ params, commit, queryRef }: Props) {
  const goTo = useNavigateTo();
  const query = useDraft(params.q, (q) => commit({ q }, "replace"));
  const [source, setSource] = useState<Source>("all");
  const [uploading, setUploading] = useState(false);
  const text = params.q.trim();
  const searching = text !== "";

  // The list of docs, until there is a query.
  const browse = useCursorPages({ ...docListQuery({ ...(source === "all" ? {} : { source }), limit: PAGE_SIZE }), enabled: !searching });
  const browseState = useQueryState(cursorPagesSource(browse), { isEmpty: (items) => items.length === 0 });

  // The best matches for the query. The server has no source filter for them, so the source is told from each hit's path.
  const { source: found } = useOneShot(searching ? { key: JSON.stringify(["docs", text]), load: (signal) => searchDocs({ q: text, limit: BEST }, { signal }) } : null);
  const foundState = useQueryState(found, { isEmpty: (r) => r.hits.filter((h) => source === "all" || docSource(h.path) === source).length === 0 });
  const hits = useMemo(() => (foundState.data?.hits ?? []).filter((h) => source === "all" || docSource(h.path) === source), [foundState.data, source]);
  const terms = useMemo(() => text.split(/\s+/).filter(Boolean), [text]);
  const docs = useMemo(() => new Set(hits.map((h) => h.doc)).size, [hits]);

  const columns = useMemo<Column<DocSummary>[]>(
    () => [
      {
        id: "title",
        header: "Title",
        width: "minmax(0,1.2fr)",
        cell: (doc) => (
          <AppLink route="doc-view" search={{ path: doc.path }} className="font-medium">
            <SafeText text={doc.title} />
          </AppLink>
        ),
      },
      { id: "path", header: "Path", width: "minmax(0,1.6fr)", cell: (doc) => <SafeText text={doc.path} className="font-mono text-xs" /> },
      { id: "from", header: "From", width: "96px", cell: (doc) => <SourceChip source={doc.source} /> },
      { id: "version", header: "Version", width: "72px", align: "right", cell: (doc) => <span className="font-mono text-xs">{`v${doc.version}`}</span> },
      { id: "size", header: "Size", width: "72px", align: "right", cell: (doc) => <span className="text-xs">{formatBytes(doc.size)}</span> },
      { id: "updated", header: "Updated", width: "112px", cell: (doc) => <span className="text-xs text-muted-foreground">{formatRelative(doc.updated_at, Date.now())}</span> },
    ],
    [],
  );

  const open = (path: string): void => {
    const href = docHref(path, null);
    if (href !== undefined) goTo(href);
  };

  let summary: string;
  let note: string;
  if (!searching) {
    const shown = browseState.data?.length ?? 0;
    summary = browseState.state === "loading" ? "Loading…" : browseState.state === "error" ? "Not loaded" : `${browse.hasMore ? `${shown}+` : pluralise(shown, "doc", "docs")}${browse.hasMore ? " docs" : ""}`;
    note = "Type to search by title, heading and text";
  } else if (foundState.state === "loading") {
    summary = "Searching…";
    note = "Best matches first, from titles, headings and text";
  } else if (foundState.state === "error") {
    summary = "Search failed";
    note = "";
  } else {
    summary = `${pluralise(hits.length, "section", "sections")} in ${pluralise(docs, "doc", "docs")}`;
    note = "Best matches first, from titles, headings and text";
  }

  const suggestions: Suggestion[] = [
    ...(terms.length > 1 ? [{ id: "fewer", label: `Use fewer words: “${visibleText(terms[0] ?? "")}”`, action: "Search", onApply: () => commit({ q: terms[0] ?? "" }, "push") }] : []),
    { id: "text", label: `Search file text for “${visibleText(text)}”`, note: "Config files in every swimlane", action: "Open Text", onApply: () => commit({ tab: "text" }, "push") },
    ...(source !== "all" ? [{ id: "sources", label: "Search all sources", note: "Git and uploaded", action: "Widen", onApply: () => setSource("all") }] : []),
  ];

  let body;
  if (!searching) {
    body = (
      <QueryBoundaryView
        state={browseState}
        noun="docs"
        source="the docs"
        staleBanner={false}
        className="flex min-h-0 flex-1 flex-col"
        loading={<DataTable label="Docs" rows={[]} columns={columns} rowKey={(doc) => String(doc.id)} state="loading" className="min-h-0 flex-1" />}
        empty={
          <EmptyState icon={BookOpen} title="No docs yet" className="pt-10">
            Docs from Git appear here once the repository is synced. An Editor can upload a Markdown doc.
          </EmptyState>
        }
      >
        {(items) => (
          <DataTable
            label="Docs"
            rows={items}
            columns={columns}
            rowKey={(doc) => String(doc.id)}
            onActivate={(doc) => open(doc.path)}
            className="min-h-0 flex-1"
            footer={{ hasMore: browse.hasMore, onLoadMore: () => void browse.loadMore(), loading: browse.isFetching, truncated: browse.truncated, step: PAGE_SIZE }}
          />
        )}
      </QueryBoundaryView>
    );
  } else {
    body = (
      <QueryBoundaryView
        state={foundState}
        noun="doc sections"
        source="the docs index"
        staleBanner={false}
        className="flex min-h-0 flex-1 flex-col"
        loading={
          <ul className="m-0 list-none p-0" aria-hidden="true">
            {[0, 1, 2, 3].map((n) => (
              <li key={n} className="flex h-[88px] flex-col gap-2 border-b border-border px-3 py-3">
                <span className="h-4 w-1/3 animate-lk-pulse rounded-sm bg-muted" />
                <span className="h-3 w-1/2 animate-lk-pulse rounded-sm bg-muted" />
                <span className="h-3 w-5/6 animate-lk-pulse rounded-sm bg-muted" />
              </li>
            ))}
          </ul>
        }
        empty={
          <div className="flex flex-col items-center gap-4 px-6 py-10">
            <EmptyState icon={BookOpen} title={`No docs match “${visibleText(text)}”`} className="p-0">
              {`Best matches come from titles, headings and text${source === "all" ? "" : `, in ${source === "git" ? "Git" : "uploaded"} docs only`}.`}
            </EmptyState>
            <SuggestionList items={suggestions} />
          </div>
        }
      >
        {() => (
          <>
            {(foundState.data?.hits.length ?? 0) >= BEST ? <TruncationBanner shown={BEST} noun="sections" /> : null}
            <div className="min-h-0 flex-1 overflow-auto">
              <ul aria-label="Doc sections" className="m-0 list-none p-0">
                {hits.map((hit, i) => (
                  <Section key={`${hit.doc}:${hit.path}:${hit.anchor ?? ""}:${i}`} hit={hit} terms={terms} />
                ))}
              </ul>
            </div>
          </>
        )}
      </QueryBoundaryView>
    );
  }

  return (
    <>
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted p-2">
        <SearchInput
          ref={queryRef}
          aria-label="Docs query"
          maxLength={MAX_QUERY}
          placeholder="Search docs"
          kbd="/"
          value={query.value}
          onChange={(event) => query.set(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") query.flush();
          }}
        />
        <ToggleGroup
          type="single"
          value={source}
          aria-label="Source"
          className="shrink-0 gap-0 overflow-hidden border-input bg-background p-0"
          onValueChange={(next) => {
            // Choosing the chosen one again would clear the choice; "All sources" is the way to clear it.
            if (next === "all" || next === "git" || next === "upload") setSource(next);
          }}
        >
          <ToggleGroupItem value="all" className={SEGMENT}>
            All sources
          </ToggleGroupItem>
          <ToggleGroupItem value="git" className={SEGMENT}>
            Git
          </ToggleGroupItem>
          <ToggleGroupItem value="upload" className={SEGMENT}>
            Uploaded
          </ToggleGroupItem>
        </ToggleGroup>
        <GatedButton variant="secondary" minRole="editor" onClick={() => setUploading(true)}>
          <Upload aria-hidden="true" />
          Upload doc
        </GatedButton>
      </div>
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-border px-3 text-xs text-muted-foreground">
        <strong aria-live="polite" className="font-semibold tabular-nums text-foreground">
          {summary}
        </strong>
        {note === "" ? null : <span>{note}</span>}
      </div>
      {body}
      <UploadDocDialog
        open={uploading}
        onOpenChange={setUploading}
        mode={{ kind: "create" }}
        onDone={(doc) => {
          open(doc.path);
        }}
      />
    </>
  );
}
