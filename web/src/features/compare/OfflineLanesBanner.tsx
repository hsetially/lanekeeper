import type { ReactNode } from "react";
import { Banner } from "@/components/app/Banner";
import { Button } from "@/components/ui/button";
import { RefreshCw, WifiOff } from "@/icons";

/**
 * The notice above a comparison when the agent behind a column is offline (States O06): which agents, and one Retry. The words under
 * the title are the screen's own, because the lanes view and the grid say different things about how old the files are.
 */
export function OfflineLanesBanner({ lanes, onRetry, children }: { lanes: readonly { readonly id: string }[]; onRetry: () => void; children: ReactNode }) {
  const first = lanes[0];
  if (first === undefined) return null;
  return (
    <Banner
      variant="warning"
      icon={<WifiOff aria-hidden="true" className="size-3.5" />}
      title={lanes.length === 1 ? `${first.id} agent is offline` : `${lanes.length} agents are offline`}
      action={
        <Button variant="secondary" size="sm" onClick={onRetry}>
          <RefreshCw data-icon="inline-start" />
          {lanes.length === 1 ? `Retry ${first.id}` : "Retry"}
        </Button>
      }
    >
      {children}
    </Banner>
  );
}
