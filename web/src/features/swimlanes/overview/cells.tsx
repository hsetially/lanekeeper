import type { SwimlaneSummary } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { DRIFT_VARIANT } from "@/components/app/drift";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge, STATUS_VARIANTS } from "@/components/app/StatusBadge";
import { TierChip } from "@/components/app/TierBadge";
import { PickupCounts } from "@/components/pickup";
import { Button } from "@/components/ui/button";
import { TenantSet } from "@/features/shell/Tenants";
import { Check, LoaderCircle, ShieldCheck } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import { agentView, offlineSince } from "../agent";
import { baselineView } from "../baseline";
import { driftChips, versionLabel } from "../format";

// The cells of the swimlane list, one component each, so that the live tests and the other screens that show a swimlane's
// drift or agent say it with the same words. A cell shows what the contract carries and nothing it does not: a swimlane
// that has no tenant yet shows "No tenants", a baseline the server did not state shows nothing.

export function SwimlaneCell({ lane }: { lane: SwimlaneSummary }) {
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      {lane.tier ? <TierChip tier={lane.tier} /> : null}
      <AppLink route="swimlane" params={{ id: lane.id }} className="min-w-0 truncate font-mono text-xs font-medium text-foreground no-underline hover:underline">
        <SafeText text={lane.display_name} />
      </AppLink>
    </span>
  );
}

export function ProjectCell({ lane }: { lane: SwimlaneSummary }) {
  return lane.project ? <SafeText text={lane.project} className="text-muted-foreground" /> : <span className="text-muted-foreground">—</span>;
}

export function BranchCell({ lane }: { lane: SwimlaneSummary }) {
  return <TenantSet tenants={lane.tenants} className="text-foreground" />;
}

export function VersionCell({ lane }: { lane: SwimlaneSummary }) {
  if (baselineView(lane.baseline).kind === "unconfirmed") return <span className="font-mono text-xs text-muted-foreground">unconfirmed</span>;
  return <SafeText text={versionLabel(lane.base_version)} className="font-mono text-xs" />;
}

export function AgentCell({ lane, now }: { lane: SwimlaneSummary; now: number }) {
  const view = agentView(lane.agent);
  const since = offlineSince(lane.agent, lane.last_scan_at);
  const text = since === null ? view.label : `${view.label} · ${formatRelative(since, now)}`;
  return (
    <span title={since === null ? undefined : `Last scan ${formatTime(since)}`} className={cn("flex min-w-0 items-center gap-1.5 text-xs", !view.connected && "text-muted-foreground")}>
      <AgentDot state={view.state} />
      <span className="min-w-0 truncate">{text}</span>
    </span>
  );
}

/** What a stale row says it last knew, in the design's words: "3 NFS ahead", "2 drift", "in sync". */
function lastKnown(lane: SwimlaneSummary): string {
  const chips = driftChips(lane.drift_counts);
  if (chips.length === 0) return lane.last_scan_at === null ? "nothing yet" : "in sync";
  const [only] = chips;
  if (chips.length === 1 && only !== undefined) return `${only.count} ${STATUS_VARIANTS[DRIFT_VARIANT[only.state]].label}`;
  return `${chips.reduce((sum, chip) => sum + chip.count, 0)} drift`;
}

export function DriftCell({ lane }: { lane: SwimlaneSummary }) {
  const connected = agentView(lane.agent).connected;
  const chips = driftChips(lane.drift_counts);
  const syncing = lane.syncing;
  return (
    <span className="flex min-w-0 items-center gap-1 overflow-hidden">
      {!connected ? (
        <span title={`Stale · last known ${lastKnown(lane)}`} className="min-w-0 truncate text-muted-foreground">
          Stale · last known {lastKnown(lane)}
        </span>
      ) : chips.length === 0 ? (
        <span className="inline-flex items-center gap-1 text-muted-foreground">
          <Check aria-hidden="true" className="size-3" />
          In sync
        </span>
      ) : (
        chips.map((chip) => <StatusBadge key={chip.state} variant={DRIFT_VARIANT[chip.state]} count={chip.count} compact />)
      )}
      {syncing ? (
        <span
          title={`A sync window is open: ${syncing.job.name} since ${formatTime(syncing.since)}. Alerts are held until it closes.`}
          className="inline-flex h-5 shrink-0 items-center gap-1 rounded-sm border border-info-border bg-info-bg px-1.5 text-xs font-medium text-info-fg"
        >
          <LoaderCircle aria-hidden="true" className="size-3 animate-lk-spin" />
          Syncing
        </span>
      ) : null}
    </span>
  );
}

export function PickupCell({ lane }: { lane: SwimlaneSummary }) {
  return <PickupCounts counts={lane.pickup_counts} density="compact" whenNone="zero" />;
}

export function FindingsCell({ lane }: { lane: SwimlaneSummary }) {
  return <span className={cn("tabular-nums", lane.finding_count > 0 ? "font-medium text-foreground" : "text-muted-foreground")}>{lane.finding_count}</span>;
}

export function BaselineCell({ lane, isAdmin }: { lane: SwimlaneSummary; isAdmin: boolean }) {
  const view = baselineView(lane.baseline);
  if (view.kind === "unknown") return <span className="text-muted-foreground">—</span>;
  if (view.kind === "confirmed") {
    const title = view.by !== null && view.at !== null ? `Confirmed by ${view.by}, ${formatTime(view.at)}` : view.at !== null ? `Confirmed ${formatTime(view.at)}` : undefined;
    return (
      <span title={title} className="inline-flex items-center gap-1 text-muted-foreground">
        <ShieldCheck aria-hidden="true" className="size-3" />
        Confirmed
      </span>
    );
  }
  const open = view.kind === "in_progress";
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      {/* The badge may shrink (its label ends in an ellipsis) so that the button after it keeps its place: the cell cuts what runs past it. */}
      <StatusBadge variant="locked" label={open ? "Adoption in progress" : "Adoption needed"} className="min-w-0 shrink" />
      {isAdmin ? (
        <Button asChild size="sm" variant="secondary" className="h-[22px]">
          <AppLink route="adopt" params={{ id: lane.id }}>
            {open ? "Continue" : "Adopt"}
          </AppLink>
        </Button>
      ) : null}
    </span>
  );
}

export function LastScanCell({ lane, now, changed }: { lane: SwimlaneSummary; now: number; changed: boolean }) {
  if (lane.last_scan_at === null) return <span className="text-muted-foreground">never</span>;
  return (
    <span title={formatTime(lane.last_scan_at)} data-changed={changed ? "true" : undefined} className={cn("whitespace-nowrap", changed && "font-semibold text-info-fg")}>
      {formatRelative(lane.last_scan_at, now)}
    </span>
  );
}
