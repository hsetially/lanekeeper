import { useRouter } from "@tanstack/react-router";
import type { AppRouter } from "@/router";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Proposal } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CircleCheck, RefreshCw } from "@/icons";
import { useShortcut } from "@/keyboard";
import { useCursorPages, type CursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatRelative } from "@/lib/time";
import { useLiveStatus } from "@/live/LiveProvider";
import { useStableList } from "@/live/stable-list";
import { useNow } from "@/live/use-now";
import { useNavigateTo, useReplaceTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import { lifetimeHours, queueParams, TABS, TAB_LABEL, tabOfSearch, type Tab } from "./model";
import { usePeople } from "./people";
import { ProposalDrawer } from "./ProposalDrawer";
import { QueueTable, ROW_HEIGHT } from "./QueueTable";
import { proposalListQuery } from "./queries";
import { rememberRow, returnToQueue, rowOnReturn } from "./restore";

// The approvals screen (Approvals page and the proposal drawer, one component, because the drawer is part of the page): the title and
// what is waiting, the four tabs, the queue, the foot, and the drawer of the proposal in the address.
//
// Pending and Stale are always asked for: the first is what the sidebar badge counts (the same request) and the second is what
// needs attention. Expired and Decided are asked for when they are opened, because each is a page of diffs the Admin may never
// want, so their tabs show no count before that. Decided is the contract's `decided=true`.
//
// The queue is held still while it is read (the handoff's stable-view rule): a proposal that arrives or goes is counted in a chip
// until the person asks for it, then shown tinted with New.

const keyOfProposal = (p: Proposal): string => String(p.id);

function QueueSkeleton() {
  return (
    <div data-slot="queue-skeleton" className="flex flex-col">
      {Array.from({ length: 6 }, (_, row) => (
        <div key={row} className="flex items-center gap-3 border-b border-border px-3" style={{ height: ROW_HEIGHT }}>
          <Skeleton className="h-3 w-24" />
          <Skeleton className="h-3 w-24" />
          <Skeleton className="h-3 w-10" />
          <Skeleton className="h-3 w-48" />
          <Skeleton className="h-3 w-10" />
          <Skeleton className="h-3 w-14" />
        </div>
      ))}
    </div>
  );
}

/** "6", or "25+" when the hub has more pages than were read. */
function countOf(pages: CursorPages<Proposal>): string | null {
  if (pages.status !== "success") return null;
  return pages.hasMore || pages.truncated ? `${pages.items.length}+` : String(pages.items.length);
}

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;

type BodyProps = {
  tab: Tab;
  rows: readonly Proposal[];
  selectedId: number | null;
  me: string | null;
  people: ReturnType<typeof usePeople>;
  onOpen: (proposal: Proposal) => void;
  /** More rows exist on the hub than were read. */
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => Promise<void>;
  /** The proposal whose drawer was open before the queue was drawn anew: the keyboard cursor starts on its row. */
  cursorOn: number | null;
};

/** The queue and its chip. It mounts when the first rows are there, so the rows it holds still are the first ones that were read. */
function QueueBody({ tab, rows, selectedId, me, people, onOpen, hasMore, loadingMore, onLoadMore, cursorOn }: BodyProps) {
  const now = useNow(true, 30_000);
  const stable = useStableList(rows, keyOfProposal);
  const [fresh, setFresh] = useState<ReadonlySet<number>>(() => new Set());
  const show = (): void => {
    const held = new Set(stable.shown.map((p) => p.id));
    setFresh((current) => new Set([...current, ...rows.filter((p) => !held.has(p.id)).map((p) => p.id)]));
    stable.apply();
  };
  // Rows the person asked for with Load more are not a change under their pointer: they are shown as soon as they are there. `asked`
  // is how many rows were shown when they asked.
  const [asked, setAsked] = useState<number | null>(null);
  const { apply } = stable;
  const grown = asked !== null && rows.length > asked;
  useEffect(() => {
    if (!grown) return;
    apply();
    setAsked(null);
  }, [grown, apply]);
  const more = (): void => {
    setAsked(stable.shown.length);
    void onLoadMore();
  };
  return (
    <>
      {stable.pending > 0 ? (
        <div role="status" className="flex h-8 flex-none items-center gap-2 border-b border-border bg-info-bg px-3 text-xs text-info-fg">
          <span className="tabular-nums">{plural(stable.pending, "update", "updates")}</span>
          <span>The queue is held still while you read it.</span>
          <Button variant="secondary" size="sm" onClick={show} className="ml-auto">
            <RefreshCw data-icon="inline-start" />
            Show
          </Button>
        </div>
      ) : null}
      <QueueTable tab={tab} rows={stable.shown} selectedId={selectedId} fresh={fresh} me={me} people={people} now={now} onOpen={onOpen} moreRows={hasMore} cursorOn={cursorOn} className="flex-1" />
      {hasMore ? (
        <div className="flex h-9 flex-none items-center gap-3 border-t border-border px-3 text-xs text-muted-foreground">
          <span className="tabular-nums">Showing {stable.shown.length} so far</span>
          <Button variant="secondary" size="sm" loading={loadingMore} onClick={more}>
            Load more
          </Button>
        </div>
      ) : null}
    </>
  );
}

type Props = {
  tab: Tab;
  /** The proposal whose drawer is open, from the address. */
  selectedId: number | null;
};

export function ApprovalsScreen({ tab, selectedId }: Props) {
  const session = useSession();
  const me = session.status === "ready" ? session.me.id : null;
  const myName = session.status === "ready" ? session.me.display_name : "";
  const people = usePeople();
  const navigate = useNavigateTo();
  const replace = useReplaceTo();
  const router = useRouter<AppRouter>();
  // Read once, when the screen is drawn: the row to put the cursor on if the person comes back from a drawer.
  const [cursorOn] = useState<number | null>(() => (selectedId === null ? rowOnReturn(router.history) : null));
  const now = useNow(true, 30_000);
  const live = useLiveStatus();

  const pending = useCursorPages({ ...proposalListQuery(queueParams("pending")) });
  const stale = useCursorPages({ ...proposalListQuery(queueParams("stale")) });
  const expired = useCursorPages({ ...proposalListQuery(queueParams("expired")), enabled: tab === "expired" });
  const decided = useCursorPages({ ...proposalListQuery(queueParams("decided")), enabled: tab === "decided" });
  const active = { pending, stale, expired, decided }[tab];
  const query = useQueryState(cursorPagesSource(active), { isEmpty: (items) => items.length === 0 });

  const hrefOf = (to: Tab): string => linkTo("approvals", undefined, to === "pending" ? {} : { state: to });
  const goTab = (next: string): void => {
    const to = tabOfSearch(next);
    if (selectedId === null) replace(hrefOf(to));
    else navigate(hrefOf(to));
  };
  const closeDrawer = (): void => {
    returnToQueue(router.history, hrefOf(tab), selectedId);
    // Closing the drawer is a navigation (it has an address), and the shell puts focus on the heading of the page after one. The
    // drawer is a dialog opened from the queue, so focus goes back to the queue: once the page has rendered, after the shell.
    const off = router.subscribe("onRendered", () => {
      off();
      document.querySelector<HTMLElement>('[role="grid"][aria-label="Proposals"]')?.focus({ preventScroll: true });
    });
    setTimeout(off, 1500);
  };
  const open = (proposal: Proposal): void => {
    rememberRow(router.history, proposal.id);
    navigate(linkTo("approval", { id: String(proposal.id) }));
  };

  const box = useRef<HTMLDivElement>(null);
  useShortcut("approvals.close-drawer-reject-needs-a", closeDrawer, { enabled: selectedId !== null });
  // The arrow keys from anywhere on the page start moving through the queue; once it has focus it moves itself, and Enter (which
  // it handles itself too) opens the proposal under the cursor. Inside the drawer the arrows are the drawer's own.
  useShortcut("approvals.move-and-open-the-drawer", (hit) => {
    const table = box.current?.querySelector<HTMLElement>('[role="grid"][aria-label="Proposals"]');
    const inside = document.activeElement;
    if (!table || hit.binding > 1 || table.contains(inside) || (inside instanceof Element && inside.closest('[aria-label="Proposal detail"]') !== null)) return false;
    table.focus();
    table.dispatchEvent(new KeyboardEvent("keydown", { key: hit.event.key, bubbles: true, cancelable: true }));
    return undefined;
  });

  const loaded = pending.status === "success" && stale.status === "success";
  const nothingWaiting = loaded && pending.items.length === 0 && stale.items.length === 0;
  const counts: Record<Tab, string | null> = { pending: countOf(pending), stale: countOf(stale), expired: tab === "expired" ? countOf(expired) : null, decided: tab === "decided" ? countOf(decided) : null };
  const subtitle =
    pending.status === "pending"
      ? "Loading…"
      : pending.status === "error"
        ? "The queue could not be read"
        : nothingWaiting
          ? "Nothing waiting"
          : `${counts.pending ?? "0"} waiting for an Admin${stale.status === "success" ? ` · ${counts.stale ?? "0"} stale` : ""}${myName === "" ? "" : ` · you are signed in as ${myName}`}`;
  const shown = active.status === "success" ? plural(active.items.length, "proposal", "proposals") + (active.hasMore ? " so far" : "") : active.status === "pending" ? "Loading…" : "";
  const lifetime = useMemo(() => {
    const first = active.items[0];
    return first === undefined ? null : lifetimeHours(first);
  }, [active.items]);
  const updated = active.dataUpdatedAt;
  const liveText = live.status === "live" ? `Live · updated ${updated > 0 ? formatRelative(updated, now) : "just now"}` : live.status === "offline" || live.status === "reconnecting" || live.status === "stopped" ? "Live updates paused" : null;

  return (
    <div ref={box} className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Approvals</h1>
          <span className="text-muted-foreground">{subtitle}</span>
        </div>
        {liveText === null ? null : (
          <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <span aria-hidden="true" className={live.status === "live" ? "size-2 rounded-full bg-success-fg" : "size-2 rounded-full bg-warning-fg"} />
            {liveText}
          </span>
        )}
      </div>
      <div className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card" aria-busy={query.state === "loading" || undefined}>
        <Tabs value={tab} onValueChange={goTab} activationMode="manual" className="min-h-0 flex-1">
          <div className="relative z-[3] flex h-10 flex-none items-center gap-2 border-b border-border bg-muted px-2">
            <TabsList aria-label="Approval status" className="inline-flex items-stretch gap-0 overflow-hidden rounded-sm border border-input bg-background">
              {TABS.map((id) => (
                <TabsTrigger
                  key={id}
                  value={id}
                  className="mb-0 h-[26px] gap-1.5 rounded-none border-0 border-l border-input px-2.5 text-sm text-foreground first:border-l-0 hover:bg-accent data-[state=active]:border-input data-[state=active]:bg-secondary data-[state=active]:text-foreground"
                >
                  {TAB_LABEL[id]}
                  {counts[id] === null ? null : (
                    <>
                      {" "}
                      <span className="font-normal text-muted-foreground tabular-nums">{counts[id]}</span>
                    </>
                  )}
                </TabsTrigger>
              ))}
            </TabsList>
            <span className="ml-auto text-xs text-muted-foreground tabular-nums">{shown}</span>
          </div>
          <TabsContent key={tab} value={tab} className="flex min-h-0 flex-1 flex-col">
            {active.truncated ? <TruncationBanner shown={active.items.length} noun="proposals" /> : null}
            <QueryBoundaryView
              state={query}
              noun="proposals"
              source="the approval queue"
              className="flex min-h-0 flex-1 flex-col"
              loading={<QueueSkeleton />}
              empty={
                <EmptyState icon={CircleCheck} title={tab === "pending" && nothingWaiting ? "No proposals" : `Nothing in ${TAB_LABEL[tab]}`}>
                  {tab === "pending" && nothingWaiting ? "When someone proposes an edit, upload, delete, restart or pull request, it appears here as it happens." : "This list updates as proposals change."}
                </EmptyState>
              }
            >
              {(rows) => <QueueBody tab={tab} rows={rows} selectedId={selectedId} me={me} people={people} onOpen={open} hasMore={active.hasMore} loadingMore={active.isFetching} onLoadMore={active.loadMore} cursorOn={cursorOn} />}
            </QueryBoundaryView>
          </TabsContent>
        </Tabs>
        <div className="flex min-h-7 flex-none flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
          <span className="tabular-nums">{shown}</span>
          {lifetime === null ? null : <span>Proposals expire {plural(lifetime, "hour", "hours")} after they are made</span>}
          <span>You can't approve your own proposal</span>
          <span className="ml-auto">Click a row to review it</span>
        </div>
        {selectedId === null ? null : <ProposalDrawer key={selectedId} id={selectedId} initial={active.items.find((p) => p.id === selectedId)} me={me} people={people} onClose={closeDrawer} onStale={() => void active.refetch()} />}
      </div>
    </div>
  );
}
