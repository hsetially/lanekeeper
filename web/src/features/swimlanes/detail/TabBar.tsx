import type { SwimlaneDetail } from "@/api/generated/model";
import { driftSummary } from "@/components/app/drift";
import { TabsList, TabsTrigger, TabCount } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Clock } from "@/icons";
import { formatClock } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";
import { offlineSince } from "../agent";

// The four tabs of a swimlane: Services, Drift, Findings and Activity. They are ROUTES (each its own address, so a link and
// Back work), drawn as the design's tab bar, with a count where the swimlane's own numbers have one. Activity is Admin-only (the
// contract's only audit read is, W11), so for anyone else the tab stays where it is, says why, and does nothing.

export type TabId = "services" | "drift" | "findings" | "activity";

/** The tab list. It is drawn inside the `Tabs` of the frame, which holds the current tab. */
export function TabBar({ id, lane, isAdmin }: { id: string; lane: SwimlaneDetail | undefined; isAdmin: boolean }) {
  const counts: Record<TabId, number | null> = {
    services: lane === undefined ? null : lane.services.length,
    drift: lane === undefined ? null : driftSummary(lane.drift_counts).total,
    findings: lane === undefined ? null : lane.finding_count,
    activity: null,
  };
  // An agent that is offline: everything below is what it reported last, and the bar says when ("as of 14:05").
  const asOf = lane === undefined ? null : offlineSince(lane.agent, lane.last_scan_at);
  return (
    <TabsList aria-label="Swimlane" className="min-h-9 flex-none gap-1 px-3">
        <TabsTrigger value="services" asChild>
          <AppLink route="services" params={{ id }} className="px-2 no-underline">
            Services {counts.services === null ? null : <TabCount className="rounded-sm border border-border bg-muted px-1.5">{counts.services}</TabCount>}
          </AppLink>
        </TabsTrigger>
        <TabsTrigger value="drift" asChild>
          <AppLink route="drift" params={{ id }} className="px-2 no-underline">
            Drift {counts.drift === null ? null : <TabCount className="rounded-sm border border-border bg-muted px-1.5">{counts.drift}</TabCount>}
          </AppLink>
        </TabsTrigger>
        <TabsTrigger value="findings" asChild>
          <AppLink route="swimlane-findings" params={{ id }} className="px-2 no-underline">
            Findings {counts.findings === null ? null : <TabCount className="rounded-sm border border-border bg-muted px-1.5">{counts.findings}</TabCount>}
          </AppLink>
        </TabsTrigger>
        {isAdmin ? (
          <TabsTrigger value="activity" asChild>
            <AppLink route="activity" params={{ id }} className="px-2 no-underline">
              Activity
            </AppLink>
          </TabsTrigger>
        ) : (
          <Tooltip>
            <TooltipTrigger asChild>
              <TabsTrigger value="activity" aria-disabled="true" className="px-2 opacity-50" onClick={(event) => event.preventDefault()}>
                Activity
              </TabsTrigger>
            </TooltipTrigger>
            <TooltipContent>Needs the Admin role. Ask an Admin to grant it.</TooltipContent>
          </Tooltip>
        )}
      {asOf === null ? null : (
        <span className="mr-0 ml-auto inline-flex h-5 items-center gap-1 self-center rounded-sm border border-dashed border-input px-1.5 text-xs font-medium text-muted-foreground">
          <Clock aria-hidden="true" className="size-3" />
          as of {formatClock(asOf)}
        </span>
      )}
    </TabsList>
  );
}
