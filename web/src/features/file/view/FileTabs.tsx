import type { ReactNode } from "react";
import { Tabs, TabsContent, TabCount, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { parseTab, type Tab } from "../model";

// NFS, Git, Baseline and Diff (File, the tab bar). Radix Tabs gives the roles, the roving focus and the arrow keys; the
// selected tab is the page's `tab` search value, so a tab is a link and the back button goes back to the one before.

export type FileTabsProps = {
  tab: Tab;
  onTab: (tab: Tab) => void;
  /** What the Git tab says it reads: "(sit7 head)", "(v2.3.1 + 4)". */
  gitLabel: string;
  /** How many changes the Diff tab has, when the comparison is known. */
  diffCount: number | null;
  children: ReactNode;
};

const NAMES: Record<Tab, string> = { nfs: "NFS", git: "Git", baseline: "Baseline", diff: "Diff" };

export function FileTabs({ tab, onTab, gitLabel, diffCount, children }: FileTabsProps) {
  const sub: Record<Tab, string> = { nfs: "(current)", git: gitLabel, baseline: "", diff: "" };
  return (
    <Tabs value={tab} onValueChange={(value) => onTab(parseTab(value))} className="min-h-0 flex-1">
      <TabsList aria-label="Versions of the file" className="flex-none gap-1 border-t bg-background px-5">
        {(Object.keys(NAMES) as Tab[]).map((key) => (
          <TabsTrigger key={key} value={key} className="gap-1.5 px-2">
            {NAMES[key]}
            {sub[key] === "" ? null : (
              <>
                {" "}
                <span className="font-normal text-muted-foreground">{sub[key]}</span>
              </>
            )}
            {key === "diff" && diffCount !== null ? <>{" "}<TabCount className="inline-flex min-w-[18px] items-center justify-center rounded-sm border border-border bg-muted px-1 font-medium">{diffCount}</TabCount></> : null}
          </TabsTrigger>
        ))}
      </TabsList>
      <TabsContent value={tab} aria-label={NAMES[tab]} className="flex min-h-0 flex-1 flex-col">
        {children}
      </TabsContent>
    </Tabs>
  );
}
