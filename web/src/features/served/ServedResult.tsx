import type { FileRole, ServedFile } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { HashChip, PathChip } from "@/components/app/chips";
import { ListSkeleton, QueryBoundaryView } from "@/components/app/QueryBoundary";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { isContentWithheld } from "@/components/masking/withheld";
import { ServedText } from "@/components/served/ServedText";
import { UnresolvedPlaceholders } from "@/components/served/UnresolvedPlaceholders";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { splitPath } from "@/features/file/model";
import { FileCode, RefreshCw, WifiOff } from "@/icons";
import type { QueryState } from "@/lib/query/use-query-state";
import { formatRelative, formatTime } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { Sources } from "./Sources";

// What the "As served" view shows for the answer to ONE request, and the states around it (the design has none: an interim design built
// from the File view's header and status bar, the States index's words, and the shared query boundary). From the top:
//   the agent, when it is known to be offline (the hub asks the config-server through it);
//   the facts about the answer: the file, the application, the tenant and the channel the config-server was asked for, the kind of file,
//     the hash of what it sent, and WHEN it was asked, because the answer is live and nothing tells the page it changed;
//   the placeholders it left; the text, or why there is none; the files the engine reads for the selection.
// Loading, an error with its request id, and an answer that may be out of date are the query boundary's, as everywhere. "Not served" is
// the one failure that is an answer: the config-server has nothing for this selection, which is a state and not a fault.

const ROLE_LABEL: Record<FileRole, string> = { property_source: "Property source", resource: "Resource file" };

/** What the page knows of the swimlane's agent: `null` while it has not been read. */
export type AgentFacts = { readonly online: boolean };

type Props = {
  swimlane: string;
  state: QueryState<ServedFile>;
  agent: AgentFacts | null;
  /** A request is in flight, the first or a later one. */
  fetching: boolean;
  /** When the answer on screen was received, in milliseconds since the epoch. */
  asOf: number | null;
};

function AgentOffline({ swimlane }: { swimlane: string }) {
  return (
    <Banner variant="warning" icon={<WifiOff aria-hidden="true" />} title={`Agent for ${swimlane} is offline`}>
      The hub asks the config-server through the swimlane&apos;s agent, so a request may fail until it is back.
    </Banner>
  );
}

function AskAgain({ fetching, onAsk, secondary = true }: { fetching: boolean; onAsk: () => void; secondary?: boolean }) {
  return (
    <Button variant={secondary ? "secondary" : "default"} size="sm" loading={fetching} onClick={onAsk}>
      {fetching ? (
        "Asking…"
      ) : (
        <>
          <RefreshCw data-icon="inline-start" />
          Ask again
        </>
      )}
    </Button>
  );
}

function NotServed({ problem, fetching, onAsk }: { problem: ApiProblem; fetching: boolean; onAsk: () => void }) {
  return (
    <div data-slot="not-served" className="flex flex-col items-center">
      <EmptyState icon={FileCode} title="Not served for this selection" action={<AskAgain fetching={fetching} onAsk={onAsk} />}>
        The config-server has no answer for this application, tenant and channel, or the swimlane has no such file or tenant. Check the application and the channel, then ask again.
      </EmptyState>
      {problem.requestId === undefined ? null : <RequestId id={problem.requestId} className="pb-6" />}
    </div>
  );
}

function Answer({ swimlane, served, state, fetching, asOf }: { swimlane: string; served: ServedFile; state: QueryState<ServedFile>; fetching: boolean; asOf: number | null }) {
  const now = useNow(true, 30_000);
  const withheld = isContentWithheld(served);
  const name = splitPath(served.file).name;
  return (
    // The facts, the placeholders, the text and the sources scroll as one when the window is too small for them (a long list of sources and
    // placeholders at 1024 wide), and the text keeps a floor (ServedText), so that what is longer is what scrolls and the text never goes.
    <div data-slot="served-answer" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div role="group" aria-label="About this answer" className="flex min-h-10 flex-none flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-3 py-1.5">
        <PathChip path={served.file} maxChars={56} />
        <span className="text-xs text-muted-foreground">
          <SafeText text={served.application} className="font-mono text-foreground" /> · <SafeText text={served.tenant} className="font-mono text-foreground" /> ·{" "}
          {served.channel === null ? (
            "no channel"
          ) : (
            <>
              channel <SafeText text={served.channel} className="font-mono text-foreground" />
            </>
          )}
        </span>
        <Badge variant="muted">{ROLE_LABEL[served.role]}</Badge>
        {/* A fact about a file the page will not show: nothing in it is worth pasting elsewhere (as the File view's withheld card). */}
        <HashChip hash={served.hash} copy={!withheld} />
        <span className="ml-auto flex items-center gap-2">
          {asOf === null ? null : (
            <span className="text-xs text-muted-foreground">
              Asked {formatTime(asOf, { seconds: true })} ({formatRelative(asOf, now)})
            </span>
          )}
          <AskAgain fetching={fetching} onAsk={state.refetch} />
        </span>
      </div>
      {withheld ? null : <UnresolvedPlaceholders placeholders={served.unresolved_placeholders} />}
      <ServedText served={served} label={`Served text of ${visibleText(name)}`} />
      <div className="flex flex-none flex-col gap-1 border-t border-border bg-muted px-3 py-1.5 text-xs text-muted-foreground">
        {withheld || served.sources.length === 0 ? null : (
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
            <span>The engine reads</span>
            <Sources swimlane={swimlane} sources={served.sources} />
          </div>
        )}
        <p className="m-0">Comparison with the rendered view is not available yet.</p>
      </div>
    </div>
  );
}

export function ServedResult({ swimlane, state, agent, fetching, asOf }: Props) {
  const problem = state.state === "error" ? state.error : null;
  return (
    <section aria-label="The config-server's answer" className="flex min-h-0 flex-1 flex-col">
      {agent?.online === false ? <AgentOffline swimlane={swimlane} /> : null}
      {problem?.status === 404 ? (
        <NotServed problem={problem} fetching={fetching} onAsk={state.refetch} />
      ) : (
        <QueryBoundaryView
          state={state}
          noun="the served file"
          source="the config-server's answer for this selection"
          className="flex min-h-0 flex-1 flex-col"
          // The card is the colour of `muted` in the dark theme, so the blocks are `accent`: they show on it in both themes.
          loading={<ListSkeleton rows={10} columns={2} blockClassName="bg-accent" />}
        >
          {(served) => <Answer swimlane={swimlane} served={served} state={state} fetching={fetching} asOf={asOf} />}
        </QueryBoundaryView>
      )}
    </section>
  );
}
