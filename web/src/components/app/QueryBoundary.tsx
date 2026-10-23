import type { ReactNode } from "react";
import type { ApiProblem } from "@/api/problem";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { ErrorState } from "@/components/app/ErrorState";
import { RequestId } from "@/components/app/RequestId";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { formatClock, formatRelative } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useNow } from "@/live/use-now";
import { useQueryState, type QuerySource, type QueryState, type QueryStateOptions, type StaleCause } from "@/lib/query/use-query-state";

// The states of a query on screen, after the design's States index: Loading (skeleton, and a footer that names what is
// loading), Empty (what is missing and the next step, which the screen supplies), Error (the problem's title, one line, the
// request ID with copy, Retry as the only action), and the "data as of" stamp of the Offline and Paused rows.
//
// Loading, error, empty and ready REPLACE the content. Refreshing and stale never do: a screen that has data keeps showing it
// while it refreshes, when it is old, and after a refresh failed (the banner says so and offers Refresh). That the live stream is
// down is not said here: the shell says it once for the whole page.
//
// Accessibility: the loading region is `aria-busy` and named; its footer, the refreshing indicator and the stale banner are
// polite live regions (role status); an error is an alert. Motion is the app's own utility (`animate-lk-spin`), which
// base.css stops under `prefers-reduced-motion`.

type Props<T> = {
  state: QueryState<T>;
  /** What is loaded, as it reads in "Couldn't load swimlanes" and "Loading swimlanes…". */
  noun: string;
  /** What the server was asked for, as it reads in "while reading the swimlane list". Default: the noun. */
  source?: string;
  /** The screen's own skeleton (its columns, its row height), instead of the default. */
  loading?: ReactNode;
  /** The design's empty state for this screen: what is missing, and the next step the role can take. */
  empty?: ReactNode;
  /** False when the screen says the same thing in its own chrome. Default true. */
  staleBanner?: boolean;
  /** The time to measure "4 min ago" from. Tests pin it. */
  now?: number;
  className?: string;
  children: (data: T) => ReactNode;
};

const WIDTHS = ["w-3/5", "w-4/5", "w-2/5", "w-full", "w-1/2", "w-3/4"] as const;

/** The default skeleton: seven rows at the table row height, so nothing moves when the data arrives. */
export function ListSkeleton({ rows = 7, columns = 3, blockClassName }: { rows?: number; columns?: number; /** Another look for the blocks: `bg-accent` on a surface that is the colour of `muted` (a card, in the dark theme). */ blockClassName?: string }) {
  return (
    <div data-slot="list-skeleton" className="flex flex-col">
      {Array.from({ length: rows }, (_, row) => (
        <div key={row} className="flex h-8 items-center gap-4 px-4">
          {Array.from({ length: columns }, (_, column) => (
            <Skeleton key={column} className={cn("h-3", WIDTHS[(row + column) % WIDTHS.length], blockClassName)} />
          ))}
        </div>
      ))}
    </div>
  );
}

const capitalise = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);

/** One line that says what happened, by what the answer was. Never what the server wrote in `detail` (S21). */
function explain(problem: ApiProblem, source: string, retryable: boolean): string {
  if (problem.status === 0) return `The server could not be reached while reading ${source}.${retryable ? " Check the connection and retry." : ""}`;
  if (problem.status === 403) return `You do not have access to ${source}. Ask an admin to grant the role it needs.`;
  if (problem.status === 404) return `${capitalise(source)} was not found. It may have been removed.`;
  if (!retryable) return `${capitalise(source)} could not be read: the request was refused. Send the request ID to the platform team.`;
  return `The server returned an error while reading ${source}. Retry, or send the request ID to the platform team.`;
}

function ErrorPanel({ problem, noun, source, retryable, onRetry }: { problem: ApiProblem; noun: string; source: string; retryable: boolean; onRetry: () => void }) {
  return (
    <ErrorState title={`Couldn't load ${noun}`} problem={problem} {...(retryable ? { onRetry } : {})}>
      {explain(problem, source, retryable)}
    </ErrorState>
  );
}

// "Live updates paused" is the shell's to say, once, for the whole page (src/components/app/PausedBanner.tsx): a screen with three
// lists would say it four times. A boundary speaks for the two causes that are the view's own: its data is old, or the last refresh
// of it failed. The `live-paused` cause stays in the state (the write gate and the tests read it); it just has no banner here.
type OwnCause = Exclude<StaleCause, "live-paused">;
const TITLE: Record<OwnCause, string> = {
  "refetch-failed": "Couldn't refresh",
  age: "This view may be out of date",
};
// Most urgent first: it is the title.
const PRIORITY: readonly OwnCause[] = ["refetch-failed", "age"];

/** The causes this boundary's banner speaks for: the view's own, never the stream's. */
export function ownCauses(causes: readonly StaleCause[]): OwnCause[] {
  return PRIORITY.filter((cause) => causes.includes(cause));
}

function staleSentences(causes: readonly OwnCause[], error: ApiProblem | null): string[] {
  const sentences: string[] = [];
  if (causes.includes("refetch-failed")) sentences.push(`Showing the last data that loaded.${error ? ` ${error.title}.` : ""}`);
  return sentences;
}

function StaleBanner({ stale, error, onRefresh, now }: { stale: NonNullable<QueryState<unknown>["stale"]>; error: ApiProblem | null; onRefresh: () => void; now: number }) {
  const causes = ownCauses(stale.causes);
  const cause = causes[0];
  if (cause === undefined) return null;
  const sentences = staleSentences(causes, error);
  if (stale.asOf !== null) sentences.push(`Last updated ${formatClock(stale.asOf)} (${formatRelative(stale.asOf, now)}).`);
  return (
    <Banner
      variant="warning"
      title={TITLE[cause]}
      action={
        <Button variant="secondary" size="sm" onClick={onRefresh}>
          Refresh
        </Button>
      }
    >
      {sentences.join(" ")}
      {/* A refresh that failed with an answer has a request ID, as the first load's error panel does: it is what a report names. */}
      {causes.includes("refetch-failed") && error?.requestId !== undefined ? (
        <>
          {" "}
          <RequestId id={error.requestId} />
        </>
      ) : null}
    </Banner>
  );
}

/** The boundary for a state the caller already has (a test, or a screen that combines several queries). */
export function QueryBoundaryView<T>({ state, noun, source = noun, loading, empty, staleBanner = true, now, className, children }: Props<T>) {
  const speaks = state.stale !== null && ownCauses(state.stale.causes).length > 0;
  const clock = useNow(speaks && staleBanner, 30_000);
  if (state.state === "loading") {
    return (
      <div data-slot="query-loading" role="group" aria-busy="true" aria-label={`Loading ${noun}`} className={className}>
        {loading ?? <ListSkeleton />}
        <p role="status" className="flex items-center gap-2 px-4 py-2 text-xs text-muted-foreground">
          <Spinner />
          Loading {noun}…
        </p>
      </div>
    );
  }
  if (state.state === "error" && state.error) {
    return (
      <div data-slot="query-boundary" className={className}>
        <ErrorPanel problem={state.error} noun={noun} source={source} retryable={state.retryable} onRetry={state.refetch} />
      </div>
    );
  }
  const data = state.data;
  return (
    // The wrapper is positioned for the indicator, which has a place of its own: appearing and disappearing never moves the content.
    <div data-slot="query-boundary" aria-busy={state.refreshing || undefined} className={cn("relative", className)}>
      <span data-slot="refreshing" role="status" className="pointer-events-none absolute top-1 right-2 flex size-4 items-center justify-center text-muted-foreground">
        {state.refreshing ? (
          <>
            <Spinner />
            <span className="sr-only">Refreshing</span>
          </>
        ) : null}
      </span>
      {state.stale && speaks && staleBanner ? <StaleBanner stale={state.stale} error={state.error} onRefresh={state.refetch} now={now ?? clock} /> : null}
      {state.state === "empty" ? (empty ?? <DefaultEmpty noun={noun} />) : data === undefined ? null : children(data)}
    </div>
  );
}

function DefaultEmpty({ noun }: { noun: string }) {
  return <EmptyState title={`No ${noun} yet`} />;
}

type ConnectedProps<T> = Omit<Props<T>, "state"> & { query: QuerySource<T> } & QueryStateOptions<T>;

/** A query on screen: loading, error, empty or ready, with the refreshing and stale overlays. */
export function QueryBoundary<T>({ query, isEmpty, refreshDelayMs, ...props }: ConnectedProps<T>) {
  const state = useQueryState(query, { ...(isEmpty ? { isEmpty } : {}), ...(refreshDelayMs === undefined ? {} : { refreshDelayMs }) });
  return <QueryBoundaryView state={state} {...props} />;
}
