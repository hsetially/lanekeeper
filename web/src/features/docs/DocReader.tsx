import { useQuery } from "@tanstack/react-query";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { DocSummary } from "@/api/generated/model";
import { CopyLink } from "@/components/app/CopyLink";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { SanitisedDocHtml } from "@/components/app/SanitisedDocHtml";
import { Skeleton } from "@/components/ui/skeleton";
import { BookOpen, ChevronRight, FileText, GitBranch, Lock, RefreshCw, Trash2, User } from "@/icons";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { formatRelative } from "@/lib/time";
import { AppLink, useNavigateTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import type { AppRouter } from "@/router";
import { DeleteDocDialog } from "./DeleteDocDialog";
import { docFileName } from "./doc-path";
import { PROSE } from "./prose";
import { belowThePageTitle, withoutImages } from "./reader-html";
import { docByPathQuery, renderedDocQuery } from "./queries";
import { UploadDocDialog } from "./UploadDocDialog";
import { formatBytes } from "./upload";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";

// The doc reader (the Search page of the handoff, frames 1k to 1m, as a screen of its own): the table of contents, the page the
// server rendered, and what the doc is. The page is `RenderedDoc.html`, handed to the one HTML sink (SanitisedDocHtml) after
// `withoutImages`; everything else on this screen is React and every string is data (SafeText).
//
// The doc is addressed by path. The reader asks which doc has the path (its id, version, source and size) and then asks for the
// page keyed by that id, so a live `doc_indexed` event and a replace reach the page that shows the doc.

const HEADING_INDENT: Record<number, string> = { 1: "", 2: "", 3: "pl-5", 4: "pl-8", 5: "pl-11", 6: "pl-14" };

function clearMarks(container: HTMLElement): void {
  for (const old of container.querySelectorAll("[data-lk-target]")) old.removeAttribute("data-lk-target");
}

/** Show a heading of the page: scroll it into view and mark it. Returns whether the page has it. */
function reveal(container: HTMLElement | null, id: string): boolean {
  if (container === null) return false;
  clearMarks(container);
  const target = document.getElementById(id);
  if (target === null || !container.contains(target)) return false;
  target.setAttribute("data-lk-target", "");
  target.scrollIntoView({ block: "start" });
  return true;
}

function DocSkeleton() {
  return (
    <div className="flex flex-col gap-3" data-slot="doc-skeleton">
      <Skeleton className="h-5 w-40" />
      <Skeleton className="h-3.5 w-full" />
      <Skeleton className="h-3.5 w-11/12" />
      <Skeleton className="h-3.5 w-4/5" />
      <Skeleton className="mt-3 h-5 w-32" />
      <Skeleton className="h-24 w-full" />
    </div>
  );
}

function ReadOnlyNote() {
  return (
    <div role="note" className="flex items-start gap-2 rounded-md border border-status-locked-border bg-status-locked-bg px-3 py-2 text-status-locked-fg">
      <Lock aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
      <span>
        <strong className="font-semibold">Read-only.</strong> This doc is managed in Git. Edit it in the repository; the change shows up here after the next sync.
      </span>
    </div>
  );
}

function DocDetails({ summary, now }: { summary: DocSummary | undefined; now: number }) {
  if (summary === undefined) return null;
  const git = summary.source === "git";
  return (
    <aside aria-label="Doc details" className="hidden min-w-0 flex-col gap-5 overflow-auto border-l border-border p-4 xl:flex">
      <section className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">Source</span>
        <span className="flex items-center gap-1.5 font-medium">
          {git ? <GitBranch aria-hidden="true" className="size-3.5 text-muted-foreground" /> : <User aria-hidden="true" className="size-3.5 text-muted-foreground" />}
          {git ? "Git" : "Uploaded"}
        </span>
        <span className="text-xs text-muted-foreground">
          Version {summary.version} · {formatBytes(summary.size)} · updated {formatRelative(summary.updated_at, now)}
        </span>
      </section>
    </aside>
  );
}

export function DocReader({ path }: { path: string }) {
  const router = useRouter<AppRouter>();
  const goTo = useNavigateTo();
  const rawHash = useRouterState({ select: (s) => s.location.hash });
  const hash = rawHash.startsWith("#") ? rawHash.slice(1) : rawHash;

  const summaries = useCursorPages(docByPathQuery(path));
  const summaryState = useQueryState(cursorPagesSource(summaries), { isEmpty: (items) => !items.some((d) => d.path === path) });
  const summary = summaryState.data?.find((d) => d.path === path);
  const page = useQuery({ ...renderedDocQuery(summary?.id ?? 0, path), enabled: summary !== undefined });
  const pageState = useQueryState(page);

  const doc = pageState.data;
  const sink = useMemo(() => (doc === undefined ? undefined : { ...doc, html: belowThePageTitle(withoutImages(doc.html)) }), [doc]);
  const activeId = doc?.headings.some((h) => h.id === hash) === true ? hash : null;
  const prose = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (sink === undefined || prose.current === null) return;
    if (activeId === null) clearMarks(prose.current);
    else reveal(prose.current, activeId);
  }, [sink, activeId]);

  const [dialog, setDialog] = useState<"replace" | "delete" | null>(null);
  const git = summary?.source === "git";
  const title = summary?.title ?? docFileName(path);
  const now = Date.now();

  let body: ReactNode;
  if (pageState.state === "error" && pageState.error?.status === 413) {
    body = (
      <EmptyState icon={FileText} title="This doc is too large to show" className="py-8">
        The server does not render a doc of more than 2 MB of text. Read it in the repository, or replace it with smaller docs.
        {pageState.error.requestId ? <RequestId id={pageState.error.requestId} className="mt-2 flex justify-center" /> : null}
      </EmptyState>
    );
  } else {
    body = (
      <QueryBoundaryView
        state={summaryState}
        noun="doc"
        loading={<DocSkeleton />}
        empty={
          <EmptyState icon={BookOpen} title="This doc was not found" className="py-8" action={<AppLink route="search" search={{ type: "docs" }} className="text-sm underline">Back to Docs</AppLink>}>
            No doc has this path. It may have been deleted, or the address may be wrong.
          </EmptyState>
        }
      >
        {() => (
          <QueryBoundaryView state={pageState} noun="doc" loading={<DocSkeleton />}>
            {() => (sink === undefined ? null : <div ref={prose}><SanitisedDocHtml doc={sink} className={PROSE} /></div>)}
          </QueryBoundaryView>
        )}
      </QueryBoundaryView>
    );
  }

  return (
    <div data-slot="doc-reader" className="flex min-h-0 flex-1 flex-col px-6 pt-4 pb-4">
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
        <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border bg-muted px-2">
          <AppLink route="search" search={{ type: "docs" }} className="rounded-sm px-2 py-1 text-foreground no-underline hover:bg-accent hover:text-foreground">
            Docs
          </AppLink>
          <ChevronRight aria-hidden="true" className="size-3 text-muted-foreground" />
          <span className="min-w-0 truncate font-medium">
            <SafeText text={title} />
          </span>
          {summary ? (
            <span className="ml-auto flex items-center gap-2">
              {/* Links to the doc and, once a heading is chosen, to that section: built from the route, never from the address bar. */}
              {activeId === null ? null : <CopyLink href={linkTo("doc-view", undefined, { path })} fragment={activeId} what="link to this section" />}
              <CopyLink href={linkTo("doc-view", undefined, { path })} what="link to this doc" />
              <GatedButton
                variant="secondary"
                {...(git ? { gate: { allowed: false, reason: "Read-only · managed in Git" } } : { minRole: "editor" })}
                onClick={() => setDialog("replace")}
              >
                <RefreshCw aria-hidden="true" />
                Replace
              </GatedButton>
              {summary.source === "upload" ? (
                <GatedButton variant="secondary" minRole="editor" onClick={() => setDialog("delete")}>
                  <Trash2 aria-hidden="true" />
                  Delete
                </GatedButton>
              ) : null}
            </span>
          ) : null}
        </div>
        <div className="grid min-h-0 flex-1 grid-cols-1 overflow-hidden lg:grid-cols-[208px_minmax(0,1fr)] xl:grid-cols-[208px_minmax(0,1fr)_296px]">
          <nav aria-label="Table of contents" className="hidden min-w-0 overflow-auto border-r border-border bg-muted px-2 py-4 lg:block">
            <div className="px-2 pb-2 text-xs font-medium text-muted-foreground">On this page</div>
            <div className="flex flex-col gap-0.5">
              {doc === undefined
                ? [0, 1, 2, 3].map((n) => <Skeleton key={n} className="mx-2 my-2 h-3 w-3/4" />)
                : doc.headings.map((heading) => (
                    <a
                      key={heading.id}
                      href={`#${heading.id}`}
                      aria-current={heading.id === activeId ? "location" : undefined}
                      onClick={(event) => {
                        event.preventDefault();
                        reveal(prose.current, heading.id);
                        router.history.replace(`${router.state.location.href.split("#")[0] ?? ""}#${heading.id}`);
                      }}
                      className={`flex h-7 items-center rounded-sm px-2 text-foreground no-underline hover:bg-accent hover:text-foreground aria-[current=location]:bg-selection aria-[current=location]:font-medium ${HEADING_INDENT[heading.level] ?? ""}`}
                    >
                      <span className="truncate">
                        <SafeText text={heading.text} />
                      </span>
                    </a>
                  ))}
            </div>
          </nav>
          <article aria-label="Document text" {...FOCUSABLE_SCROLLER} className="min-w-0 overflow-auto px-8 py-6">
            <div className="flex max-w-[760px] flex-col gap-3">
              <div className="flex flex-col gap-1.5">
                <h1 className="m-0 text-2xl font-semibold leading-[30px] tracking-tight">
                  <SafeText text={title} />
                </h1>
                <span className="font-mono text-xs break-all text-muted-foreground">
                  <SafeText text={path} />
                </span>
              </div>
              {git ? <ReadOnlyNote /> : null}
              {body}
            </div>
          </article>
          <DocDetails summary={summary} now={now} />
        </div>
      </div>
      {summary ? (
        <>
          <UploadDocDialog open={dialog === "replace"} onOpenChange={(open) => setDialog(open ? "replace" : null)} mode={{ kind: "replace", doc: summary }} />
          {summary.source === "upload" ? (
            <DeleteDocDialog
              open={dialog === "delete"}
              onOpenChange={(open) => setDialog(open ? "delete" : null)}
              doc={summary}
              onDone={() => {
                goTo(linkTo("search", undefined, { type: "docs" }));
              }}
            />
          ) : null}
        </>
      ) : null}
    </div>
  );
}
