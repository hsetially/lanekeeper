import type { SwimlaneDetail } from "@/api/generated/model";
import { Banner } from "@/components/app/Banner";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { LoaderCircle, RefreshCw, WifiOff } from "@/icons";
import { formatClock } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";
import { AgentStatus } from "@/api/generated/model";
import { editLock } from "../baseline";
import { offlineSince } from "../agent";

// The three banners that sit above a swimlane's header, in the design's order: locked, agent offline, sync in progress. Each says
// what is the matter, what it turns off and what to do; each is a status, not a toast, and stays until the cause is gone.

/** "Editing locked": the baseline is not confirmed. The design gives it its own colour, the locked family of the Banner. */
export function LockBanner({ lane, isAdmin }: { lane: SwimlaneDetail; isAdmin: boolean }) {
  const lock = editLock(lane.display_name, lane.baseline);
  if (lock === null) return null;
  return (
    <Banner
      variant="locked"
      title={lock.title}
      action={
        isAdmin ? (
          <Button asChild size="sm" variant="ghost" className="border-current text-inherit hover:bg-transparent hover:underline">
            <AppLink route="adopt" params={{ id: lane.id }}>
              {lock.kind === "in_progress" ? "Continue adoption" : "Start adoption"}
            </AppLink>
          </Button>
        ) : (
          <span className="text-xs">Ask an admin to finish adoption.</span>
        )
      }
    >
      <SafeText text={lock.text} />
    </Banner>
  );
}

/** "Agent offline since 14:05": what is on screen is the last thing the agent reported, and restarts are off until it is back. */
export function AgentBanner({ lane, onCheck, checking }: { lane: SwimlaneDetail; onCheck: () => void; checking: boolean }) {
  if (lane.agent === AgentStatus.connected) return null;
  const since = offlineSince(lane.agent, lane.last_scan_at);
  const never = lane.agent === AgentStatus.never_seen;
  return (
    <Banner
      variant="warning"
      icon={<WifiOff aria-hidden="true" />}
      title={never ? "Agent has not reported yet" : since === null ? "Agent offline" : `Agent offline since ${formatClock(since)}`}
      action={
        <Button variant="secondary" size="sm" loading={checking} onClick={onCheck}>
          <RefreshCw data-icon="inline-start" />
          Check agent
        </Button>
      }
    >
      {since === null ? "Restarts are off until the agent connects." : `Showing data as of ${formatClock(since)}. Restarts are off until the agent reconnects.`}
    </Banner>
  );
}

/** "Sync in progress": a sync window is open, alerts are held until it closes. */
export function SyncBanner({ lane }: { lane: SwimlaneDetail }) {
  if (!lane.syncing) return null;
  return (
    <Banner variant="info" icon={<LoaderCircle aria-hidden="true" className="animate-lk-spin" />} title="Sync in progress" action={<span className="font-mono text-xs">{lane.syncing.job.name}</span>}>
      Alerts are held until the sync completes.
    </Banner>
  );
}
