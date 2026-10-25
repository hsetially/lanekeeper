import { AgentDot } from "@/components/app/AgentDot";
import { SafeText } from "@/components/app/SafeText";
import { TierChip } from "@/components/app/TierBadge";
import { isOffline, laneTitle, type LaneColumn } from "./lanes";

/** One line, as high as the table's header: the tier, the swimlane and its agent. The rest is in the tooltip and the banner. */
export function LaneHeader({ lane }: { lane: LaneColumn }) {
  const offline = isOffline(lane);
  return (
    <span title={laneTitle(lane)} data-offline={offline || undefined} className="flex min-w-0 items-center gap-1.5 data-[offline]:border-b data-[offline]:border-dashed data-[offline]:border-input">
      {lane.tier === null ? null : <TierChip tier={lane.tier} />}
      <SafeText text={lane.id} className="truncate font-mono font-medium text-foreground" />
      {lane.agent === null ? null : <AgentDot state={lane.agent.state} />}
    </span>
  );
}
