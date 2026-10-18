import type { SwimlaneSummary } from "@/api/generated/model";
import { Banner } from "@/components/app/Banner";
import { Button } from "@/components/ui/button";
import { WifiOff } from "@/icons";
import { formatClock, formatRelative } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";

// State O06 for this screen: the agent that reports on a swimlane stopped reporting, so what its findings say is as of its last
// scan. The rows stay (the last known data is still the best there is) and the banner says which swimlanes and since when. Only
// a swimlane that has findings is named: an offline agent with nothing open changes nothing on this screen. A swimlane id is plain
// text on purpose: the contract limits it to lower case letters, digits and hyphens.

const list = (ids: readonly string[]): string => ids.join(", ");

export function OfflineBanner({ lanes, now, canCheck }: { lanes: readonly SwimlaneSummary[]; now: number; canCheck: boolean }) {
  const offline = lanes.filter((lane) => lane.agent === "disconnected" && lane.finding_count > 0);
  if (offline.length === 0) return null;
  const ids = offline.map((lane) => lane.id);
  return (
    <Banner
      variant="warning"
      icon={<WifiOff />}
      title={offline.length === 1 ? `Agent for ${list(ids)} is offline` : `Agents for ${list(ids)} are offline`}
      {...(canCheck
        ? {
            action: (
              <Button asChild variant="secondary" size="sm">
                <AppLink route="admin-swimlanes">Check agent</AppLink>
              </Button>
            ),
          }
        : {})}
    >
      {offline.map((lane) => (
        <span key={lane.id} className="mr-3">
          {lane.last_scan_at === null ? `Findings for ${lane.id} have no scan time on record.` : `Findings for ${lane.id} are as of ${formatClock(lane.last_scan_at)} (${formatRelative(lane.last_scan_at, now)}).`}
        </span>
      ))}
    </Banner>
  );
}
