import { RefreshCw, WifiOff } from "@/icons";
import { Banner } from "@/components/app/Banner";
import { Button } from "@/components/ui/button";
import { secondsUntil } from "@/components/app/LiveIndicator";
import { formatClock } from "@/lib/time";
import { useLiveActions, useLiveStatus, type LiveStatusView } from "@/live/LiveProvider";
import { useNow } from "@/live/use-now";

// "Live updates paused, retrying" (Shell frame 1j, States U rows): the stream is offline, the page keeps showing the last
// data, and saving and restarts are off until it returns (the write gate says "Unavailable offline"). One reason-string table
// for the screen, so every page says the same thing.
//
// A stream that STOPPED (the hub refused it: no access, or a request that is wrong) is not retried and says why instead.

const STOPPED_REASON = {
  forbidden: "You do not have access to live updates. Changes other people make appear when you refresh.",
  bad_request: "The live connection was refused. Changes other people make appear when you refresh.",
  unauthorized: "Your session ended. Sign in again to see changes as they happen.",
} as const;

export function PausedBannerView({ view, now, onRetry }: { view: LiveStatusView; now: number; onRetry: () => void }) {
  if (view.status === "offline") {
    const seconds = secondsUntil(view.retryAt, now);
    return (
      <Banner
        variant="warning"
        icon={<WifiOff aria-hidden="true" />}
        title="Live updates paused, retrying"
        action={
          <>
            {seconds === null ? null : (
              <span aria-hidden="true" className="font-mono text-xs">
                Next attempt in {seconds}s
              </span>
            )}
            <Button variant="secondary" size="sm" onClick={onRetry}>
              <RefreshCw aria-hidden="true" />
              Retry now
            </Button>
          </>
        }
      >
        {view.dataAsOf === null ? "Showing the data from the last time this page loaded." : `Showing data from ${formatClock(view.dataAsOf)}.`} Saving and restarts are off until the connection returns.
      </Banner>
    );
  }
  if (view.status === "stopped") {
    return (
      <Banner variant="warning" icon={<WifiOff aria-hidden="true" />} title="Live updates stopped">
        {STOPPED_REASON[view.stopReason ?? "bad_request"]}
      </Banner>
    );
  }
  return null;
}

/** The banner, connected to the live stream. */
export function PausedBanner() {
  const view = useLiveStatus();
  const { retryNow } = useLiveActions();
  const now = useNow(view.status === "offline");
  return <PausedBannerView view={view} now={now} onRetry={retryNow} />;
}
