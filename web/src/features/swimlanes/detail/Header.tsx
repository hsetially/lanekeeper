import type { SwimlaneDetail } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { CopyButton } from "@/components/app/CopyButton";
import { COPY_GROUP, COPY_REVEAL } from "@/components/app/CopyValue";
import { DRIFT_VARIANT } from "@/components/app/drift";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { TierChip } from "@/components/app/TierBadge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { TenantSet } from "@/features/shell/Tenants";
import { ChevronDown, Columns2, Search, ShieldCheck, Tag } from "@/icons";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { formatRelative, formatTime } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";
import { agentView, offlineSince } from "../agent";
import { baselineView } from "../baseline";
import { driftChips, versionLabel } from "../format";
import { pickupSummary } from "../pickup";

// The head of a swimlane's screens: its tier and name, the tenants it serves (a SET: the first and a "+N"), the base version, the
// agent, the baseline, what is not in sync, and the two ways out to the other screens (compare, search). It is drawn from the
// swimlane's detail, and before that arrives it is the name from the address, so the page has its title at once.

export function Header({ id, lane, now }: { id: string; lane: SwimlaneDetail | undefined; now: number }) {
  const agent = lane ? agentView(lane.agent) : undefined;
  const since = lane ? offlineSince(lane.agent, lane.last_scan_at) : null;
  const baseline = lane ? baselineView(lane.baseline) : { kind: "unknown" as const };
  const chips = lane ? driftChips(lane.drift_counts) : [];
  const pickup = pickupSummary(lane?.pickup_counts);
  const name = lane?.display_name ?? id;
  return (
    <div className="flex flex-none flex-wrap items-start justify-between gap-x-4 gap-y-3 px-6 pt-3.5 pb-3">
      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          {lane?.tier ? <TierChip tier={lane.tier} /> : null}
          {/* The button is beside the title, not in it: a heading's name is the swimlane's, and the button's is "Copy swimlane name". */}
          <span className={`${COPY_GROUP} inline-flex max-w-full items-center gap-0.5`}>
            {/* A name has no break in it that the page can count on: it may be cut anywhere, rather than make the page scroll sideways. */}
            <h1 className="m-0 min-w-0 font-mono text-xl leading-[26px] font-medium [overflow-wrap:anywhere]">
              <SafeText text={name} />
            </h1>
            <CopyButton value={name} what="swimlane name" className={COPY_REVEAL} />
          </span>
          {lane ? (
            <span className="inline-flex h-[22px] items-center rounded-sm border border-info-border bg-info-bg px-1.5 text-info-fg">
              <TenantSet tenants={lane.tenants} className="text-info-fg" />
            </span>
          ) : null}
          {lane ? (
            <span className={baseline.kind === "unconfirmed" ? "inline-flex h-[22px] items-center gap-1 rounded-sm border border-border px-1.5 font-mono text-xs text-muted-foreground" : "inline-flex h-[22px] items-center gap-1 rounded-sm border border-border px-1.5 font-mono text-xs"}>
              <Tag aria-hidden="true" className="size-3 text-muted-foreground" />
              <SafeText text={baseline.kind === "unconfirmed" ? "unconfirmed" : versionLabel(lane.base_version)} />
            </span>
          ) : null}
          {agent ? (
            <span title={since === null ? undefined : `Last scan ${formatTime(since)}`} className="inline-flex items-center gap-1.5 text-xs">
              <AgentDot state={agent.state} />
              {since === null ? agent.label : `${agent.label} · ${formatRelative(since, now)}`}
            </span>
          ) : null}
          {baseline.kind === "confirmed" ? (
            <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
              <ShieldCheck aria-hidden="true" className="size-3" />
              Baseline confirmed
            </span>
          ) : baseline.kind === "unconfirmed" || baseline.kind === "in_progress" ? (
            <StatusBadge variant="locked" label={baseline.kind === "in_progress" ? "Adoption in progress" : "Adoption needed"} />
          ) : null}
        </div>
        {chips.length > 0 || pickup.variant !== null ? (
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
            {chips.map((chip) => (
              <StatusBadge key={chip.state} variant={DRIFT_VARIANT[chip.state]} count={chip.count} />
            ))}
            {pickup.variant !== null ? <StatusBadge variant={pickup.variant} count={pickup.actionable} /> : null}
          </div>
        ) : null}
      </div>
      <div className="flex max-w-full min-w-0 flex-wrap items-center gap-2">
        <CompareMenu id={id} />
        <Button asChild variant="secondary" className="max-w-full">
          <AppLink route="search" search={{ type: "text", lanes: id }}>
            <Search data-icon="inline-start" />
            Search in <SafeText text={name} className="min-w-0 truncate font-mono" />
          </AppLink>
        </Button>
      </div>
    </div>
  );
}

/** "Compare with…": the other swimlanes, each a link to the comparison of this one with it. */
function CompareMenu({ id }: { id: string }) {
  const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const others = pages.items.filter((lane) => lane.id !== id);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="secondary">
          <Columns2 data-icon="inline-start" />
          Compare with…
          <ChevronDown data-icon="inline-end" className="text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-h-72 min-w-48 overflow-y-auto">
        <DropdownMenuLabel>Compare {id} with</DropdownMenuLabel>
        {others.length === 0 ? <div className="px-2 py-1.5 text-sm text-muted-foreground">No other swimlane.</div> : null}
        {others.map((other) => (
          <DropdownMenuItem key={other.id} asChild>
            <AppLink route="compare" search={{ lanes: `${id},${other.id}` }}>
              <SafeText text={other.display_name} className="font-mono text-xs" />
            </AppLink>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
