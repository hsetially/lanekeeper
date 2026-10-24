import { useQuery } from "@tanstack/react-query";
import type { DocHit } from "@/api/generated/model";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { BookOpen, X } from "@/icons";
import { useQueryState } from "@/lib/query/use-query-state";
import { isPlainClick, useNavigateTo } from "@/routes/AppLink";
import { docLink, flagRow, MAX_FLAG_ROWS, type FlagRow } from "./model";
import { docsForFileQuery, fileFlagsQuery } from "./queries";

// "Docs for this file" (File, 1o): the documents that describe the file, each with a way into the reader at the right heading,
// and the table of the documented feature flags this file sets, with the value this swimlane has next to the template default.
// What the docs say is data: titles, headings and snippets are drawn as text only (a snippet the secret scan flagged arrives as
// its marker and is drawn as that marker). A doc's link goes to the reader with the path as the server gave it.

function SectionSkeleton() {
  return (
    <div className="flex flex-col gap-2">
      <Skeleton className="h-4 w-2/5" />
      <Skeleton className="h-3 w-full" />
      <Skeleton className="h-3 w-4/5" />
    </div>
  );
}

function DocCard({ hit }: { hit: DocHit }) {
  const goTo = useNavigateTo();
  const href = docLink(hit.path, hit.anchor);
  const trail = hit.heading_path.length > 0 ? hit.heading_path.join(" > ") : null;
  return (
    <li className="flex flex-col gap-1.5 rounded-md border border-border p-3">
      <div className="flex items-center gap-2">
        <BookOpen aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        <strong className="min-w-0 font-semibold [overflow-wrap:anywhere]">
          <SafeText text={hit.title} />
        </strong>
      </div>
      {trail === null ? null : (
        <span className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
          <SafeText text={trail} />
        </span>
      )}
      <span className="line-clamp-3 text-muted-foreground [overflow-wrap:anywhere]">
        <SafeText text={hit.snippet} />
      </span>
      {href === null ? (
        <span className="text-xs text-muted-foreground">This document cannot be opened from here.</span>
      ) : (
        <a
          href={href}
          rel="noopener noreferrer"
          aria-label={`Open the doc: ${hit.title}`}
          className="w-fit font-medium"
          onClick={(event) => {
            if (!isPlainClick(event)) return;
            event.preventDefault();
            goTo(href);
          }}
        >
          Open the doc
        </a>
      )}
    </li>
  );
}

function Value({ text, bold }: { text: string | null; bold: boolean }) {
  if (text === null) return <span className="font-sans text-xs text-muted-foreground italic">Not set</span>;
  return (
    <span className={bold ? "font-semibold" : undefined}>
      <SafeText text={text} />
    </span>
  );
}

function FlagsTable({ rows, truncated }: { rows: readonly FlagRow[]; truncated: boolean }) {
  return (
    <>
      <div className="overflow-hidden rounded-md border border-border">
        <table className="w-full table-fixed border-collapse text-left">
          <caption className="sr-only">Feature flags in this file</caption>
          <thead>
            <tr className="h-7 border-b border-border bg-muted text-xs font-medium text-muted-foreground">
              <th scope="col" className="px-2.5 font-medium">
                Setting
              </th>
              <th scope="col" className="w-20 px-2 font-medium">
                This file
              </th>
              <th scope="col" className="w-20 px-2 font-medium">
                Default
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={`${row.flag}:${row.setting}`} className="border-b border-border last:border-b-0">
                <td className="min-w-0 px-2.5 py-1.5 align-top">
                  <span className="block font-mono text-xs font-medium [overflow-wrap:anywhere]">
                    <SafeText text={row.setting} />
                  </span>
                  <span className="block text-xs text-muted-foreground [overflow-wrap:anywhere]">
                    <SafeText text={row.flag} />
                  </span>
                </td>
                <td className="px-2 py-1.5 align-top font-mono text-xs [overflow-wrap:anywhere]">
                  <Value text={row.value} bold={row.differs} />
                </td>
                <td className="px-2 py-1.5 align-top font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]">
                  <Value text={row.fallback} bold={false} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <span className="text-xs text-muted-foreground">Bold values differ from the default.</span>
      {truncated ? <span className="text-xs text-muted-foreground">Showing the first {MAX_FLAG_ROWS} flags.</span> : null}
    </>
  );
}

export type DocsPanelProps = {
  /** The swimlane. */
  lane: string;
  /** The logical file: the name without the tenant, which is what docs are about. Null while it is not known: nothing is asked yet. */
  file: string | null;
  onClose: () => void;
};

export function DocsPanel({ lane, file, onClose }: DocsPanelProps) {
  const docs = useQueryState(useQuery({ ...docsForFileQuery(file ?? ""), enabled: file !== null }), { isEmpty: (data) => data.hits.length === 0 });
  const flags = useQueryState(useQuery({ ...fileFlagsQuery(lane, file ?? ""), enabled: file !== null }), { isEmpty: (data) => data.flags.length === 0 });
  return (
    <aside aria-label="Docs for this file" className="flex w-[400px] max-w-full flex-none flex-col border-l border-border bg-card">
      <div className="flex h-10 flex-none items-center justify-between border-b border-border pr-3 pl-4">
        <strong className="text-base font-semibold">Docs for this file</strong>
        <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
          <X aria-hidden="true" />
        </Button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
        <section aria-label="Documents" className="flex flex-col gap-2">
          <QueryBoundaryView state={docs} noun="docs" loading={<SectionSkeleton />} empty={<p className="m-0 text-muted-foreground">No document describes this file yet.</p>} staleBanner={false}>
            {(data) => (
              <ul className="m-0 flex list-none flex-col gap-2 p-0">
                {data.hits.map((hit, i) => (
                  <DocCard key={`${hit.doc}:${hit.path}:${hit.anchor ?? ""}:${i}`} hit={hit} />
                ))}
              </ul>
            )}
          </QueryBoundaryView>
        </section>
        <section aria-label="Feature flags" className="flex flex-col gap-1.5">
          <strong className="font-semibold">Feature Flags in this file</strong>
          <QueryBoundaryView state={flags} noun="feature flags" loading={<SectionSkeleton />} empty={<p className="m-0 text-muted-foreground">No documented feature flag is set in this file.</p>} staleBanner={false}>
            {(data) => <FlagsTable rows={data.flags.slice(0, MAX_FLAG_ROWS).map((flag) => flagRow(flag, lane))} truncated={data.flags.length > MAX_FLAG_ROWS} />}
          </QueryBoundaryView>
        </section>
      </div>
    </aside>
  );
}
