import { useRouter } from "@tanstack/react-router";
import { useCallback, useEffect, useRef } from "react";
import { Kbd } from "@/components/ui/kbd";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { BookOpen, FileText, Settings } from "@/icons";
import { useShortcut } from "@/keyboard";
import type { AppRouter } from "@/router";
import { linkTo } from "@/routes/links";
import type { PageProps } from "@/routes/props";
import { DocsSearch } from "./DocsSearch";
import { useLanes } from "./lanes";
import { SettingsSearch } from "./SettingsSearch";
import type { Commit } from "./tabs";
import { TextSearch } from "./TextSearch";
import { readSearch, toSearch, type SearchTab } from "./url";

// Search (the design's Search page, frames 1a to 1t): three tabs on one screen, Text, Settings and Docs, and "/" focuses the query
// on every one. What is searched is in the address, so a search is a link and the Back button steps through searches.

const SUBTITLE: Record<SearchTab, string> = {
  text: "Find text in config files across swimlanes. Literal by default.",
  settings: "Find a setting by its path, with its value in each swimlane.",
  docs: "Search docs by title, heading and content.",
};

// The page's tabs are drawn smaller than the shared underline tabs: 32 px high, 12 px of padding, 13 px type, bold when chosen.
const TAB = "h-8 px-3 text-sm font-normal data-[state=active]:font-semibold";

export default function SearchPage({ search }: PageProps<"search">) {
  const router = useRouter<AppRouter>();
  const params = readSearch(search);
  const { lanes } = useLanes();
  const queryRef = useRef<HTMLInputElement>(null);

  // The handlers below read the parameters when they run, not when they were made: a debounced one fires after more was typed.
  const latest = useRef(params);
  useEffect(() => {
    latest.current = params;
  });
  const commit = useCallback<Commit>(
    (patch, how) => {
      const href = linkTo("search", undefined, toSearch({ ...latest.current, ...patch }));
      if (how === "replace") router.history.replace(href);
      else router.history.push(href);
    },
    [router],
  );

  useShortcut("search.focus-query", () => {
    const field = queryRef.current;
    if (field === null) return false;
    field.focus();
    field.select();
    return undefined;
  });

  return (
    <div data-slot="search-page" className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex shrink-0 flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-2">
        <div className="flex flex-col gap-1">
          <h1 className="m-0 text-xl leading-[26px] font-semibold tracking-tight">Search</h1>
          <span className="text-muted-foreground">{SUBTITLE[params.tab]}</span>
        </div>
        <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
          Focus the query <Kbd>/</Kbd>
        </span>
      </div>
      <Tabs
        value={params.tab}
        onValueChange={(value) => {
          const tab = value as SearchTab;
          commit({ tab, caseSensitive: false, regex: false, glob: "", ...(tab === "docs" ? { lanes: null } : {}) }, "push");
        }}
        className="min-h-0 flex-1"
      >
        <TabsList aria-label="Search type" className="mb-3 shrink-0 gap-1">
          <TabsTrigger value="text" className={TAB}>
            <FileText aria-hidden="true" className="size-3.5" />
            Text
          </TabsTrigger>
          <TabsTrigger value="settings" className={TAB}>
            <Settings aria-hidden="true" className="size-3.5" />
            Settings
          </TabsTrigger>
          <TabsTrigger value="docs" className={TAB}>
            <BookOpen aria-hidden="true" className="size-3.5" />
            Docs
          </TabsTrigger>
        </TabsList>
        <div className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
          <TabsContent value="text" className="flex min-h-0 flex-1 flex-col">
            <TextSearch params={params} commit={commit} lanes={lanes} queryRef={queryRef} />
          </TabsContent>
          <TabsContent value="settings" className="flex min-h-0 flex-1 flex-col">
            <SettingsSearch params={params} commit={commit} lanes={lanes} queryRef={queryRef} />
          </TabsContent>
          <TabsContent value="docs" className="flex min-h-0 flex-1 flex-col">
            <DocsSearch params={params} commit={commit} queryRef={queryRef} />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  );
}
