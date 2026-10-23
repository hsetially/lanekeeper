import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { usePanelRef } from "react-resizable-panels";
import { Role, type SwimlaneDetail } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { Skeleton } from "@/components/ui/skeleton";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { TabsContent, Tabs } from "@/components/ui/tabs";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { useShortcut } from "@/keyboard";
import { prefs } from "@/lib/prefs";
import { useQueryState } from "@/lib/query/use-query-state";
import { useNow } from "@/live/use-now";
import { AgentBanner, LockBanner, SyncBanner } from "./banners";
import { Header } from "./Header";
import { TabBar, type TabId } from "./TabBar";
import { FileTree } from "./tree/FileTree";

// What every screen of a swimlane shares: the banners that say why something is off, the head, the files on the left and the tabs
// on the right. A tab is a route, so each page renders this with its own tab and its own content. The swimlane's detail is asked
// for once here (the live events keep it fresh) and handed to the content, which is drawn only when it has arrived.

const TREE_PANEL = "swimlane-tree";
const DEFAULT_TREE_PX = 460;
const MIN_TREE_PX = 240;

function BodySkeleton() {
  return (
    <div role="status" aria-busy="true" aria-label="Loading the swimlane" className="flex flex-1 flex-col gap-2 p-4">
      {[0, 1, 2, 3, 4].map((i) => (
        <Skeleton key={i} className="h-12" />
      ))}
    </div>
  );
}

export function DetailFrame({ id, tab, noun, children }: { id: string; tab: TabId; noun: string; children: (lane: SwimlaneDetail) => ReactNode }) {
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  const isAdmin = roleAtLeast(role, Role.admin);
  const now = useNow(true, 30_000);
  const detail = useQuery(swimlaneQuery(id));
  const state = useQueryState(detail);
  const lane = state.data;
  const filterRef = useRef<HTMLInputElement>(null);
  const treePanel = usePanelRef();
  // Home on the handle collapses the tree (the keymap). A collapsed tree is inert: nothing in it takes focus or is read out, as it
  // has no width to be seen in. `f` ("Filter files") brings it back first, and the filter takes focus once the tree is reachable.
  const [collapsed, setCollapsed] = useState(false);
  const wantsFilter = useRef(false);
  // Each request for the filter is a render of its own. The tree can be collapsed (the panel says so) before the page has drawn the
  // `collapsed` the panel reported: an `f` right after Home then asks for the tree to open while `collapsed` is still false, the
  // report that the tree is open again changes nothing (false to true to false in one render), and an effect that waited for a change
  // of `collapsed` never ran. So the effect also runs for every request, and finds the tree reachable or waits for the change.
  const [filterAsks, setFilterAsks] = useState(0);
  const focusFilter = (): void => {
    filterRef.current?.focus();
    filterRef.current?.select();
  };
  useShortcut("local.detail.filter-files", () => {
    if (treePanel.current?.isCollapsed() === true) {
      wantsFilter.current = true;
      treePanel.current.expand();
      setFilterAsks((asks) => asks + 1);
      return;
    }
    focusFilter();
  });
  useEffect(() => {
    if (collapsed || !wantsFilter.current) return;
    wantsFilter.current = false;
    focusFilter();
  }, [collapsed, filterAsks]);

  const saved = useRef(prefs.get("splitWidth", TREE_PANEL) ?? DEFAULT_TREE_PX);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {lane ? (
        <>
          <LockBanner lane={lane} isAdmin={isAdmin} />
          <AgentBanner
            lane={lane}
            onCheck={state.refetch}
            checking={detail.isFetching}
          />
          <SyncBanner lane={lane} />
        </>
      ) : null}
      <Header id={id} lane={lane} now={now} />
      <div className="flex min-h-0 flex-1 border-t border-border">
        <QueryBoundaryView state={state} noun={noun} source={`${id} services`} className="flex min-h-0 flex-1 flex-col" loading={<BodySkeleton />}>
          {(data) => (
            <ResizablePanelGroup orientation="horizontal" id="swimlane-body">
              <ResizablePanel
                id={TREE_PANEL}
                panelRef={treePanel}
                defaultSize={saved.current}
                minSize={MIN_TREE_PX}
                maxSize={720}
                collapsible
                collapsedSize={0}
                inert={collapsed}
                groupResizeBehavior="preserve-pixel-size"
                onResize={(size) => {
                  setCollapsed(size.inPixels === 0);
                  clearTimeout(timer.current);
                  // A collapsed tree is not a width to come back to: the width that was kept stays.
                  if (size.inPixels < MIN_TREE_PX) return;
                  timer.current = setTimeout(() => {
                    prefs.set("splitWidth", TREE_PANEL, Math.round(size.inPixels));
                  }, 300);
                }}
              >
                <FileTree id={id} name={data.display_name} filterRef={filterRef} />
              </ResizablePanel>
              <ResizableHandle withHandle panel={treePanel} aria-label="Resize file tree" id="swimlane-handle" className="w-[7px] border-x border-border bg-muted" />
              <ResizablePanel id="swimlane-tabs" minSize={360}>
                <Tabs value={tab} className="h-full min-h-0">
                  <TabBar id={id} lane={data} isAdmin={isAdmin} />
                  <TabsContent value={tab} className="flex min-h-0 flex-1 flex-col">
                    {children(data)}
                  </TabsContent>
                </Tabs>
              </ResizablePanel>
            </ResizablePanelGroup>
          )}
        </QueryBoundaryView>
      </div>
    </div>
  );
}
