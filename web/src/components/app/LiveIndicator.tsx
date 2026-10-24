import { WifiOff } from "@/icons";
import { useLiveStatus, type LiveStatusView } from "@/live/LiveProvider";
import { useNow } from "@/live/use-now";
import { formatClock } from "@/lib/time";
import { cn } from "@/lib/utils";

// The pill in the header (Shell frames 1h, 1i, 1j): Live, Reconnecting with a countdown, Offline with the time the data is
// from. A status is never colour alone: a filled dot, a pulsing ring and a wifi-off icon differ by shape, and each has its words.
//
// A screen reader hears a stable sentence that changes only when the status does. The countdown ticks every second, so it is
// hidden from assistive technology (it would be read out once a second otherwise); the pill is a polite live region.

const PILL = "inline-flex h-6 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-2 text-xs font-medium";

export function secondsUntil(at: number | null, now: number): number | null {
  if (at === null) return null;
  const seconds = Math.ceil((at - now) / 1000);
  return seconds > 0 ? seconds : null;
}

export function LiveIndicatorView({ view, now }: { view: LiveStatusView; now: number }) {
  switch (view.status) {
    case "idle":
      return null;
    case "connecting":
      return (
        <span role="status" data-slot="live-indicator" data-status="connecting" className={cn(PILL, "border-border text-muted-foreground")}>
          <span aria-hidden="true" data-shape="ring" className="size-2 rounded-full border-2 border-agent-offline" />
          <span aria-hidden="true">Connecting…</span>
          <span className="sr-only">Connecting to live updates</span>
        </span>
      );
    case "live":
      return (
        <span role="status" data-slot="live-indicator" data-status="live" className={cn(PILL, "border-border")}>
          <span aria-hidden="true" data-shape="filled" className="size-2 rounded-full bg-agent-online" />
          <span aria-hidden="true">Live</span>
          <span className="sr-only">Live updates are on</span>
        </span>
      );
    case "reconnecting": {
      const seconds = secondsUntil(view.retryAt, now);
      return (
        <span role="status" data-slot="live-indicator" data-status="reconnecting" className={cn(PILL, "border-warning-border bg-warning-bg text-warning-fg")}>
          <span aria-hidden="true" data-shape="ring" className="size-2 animate-lk-pulse rounded-full border-2 border-agent-degraded bg-warning-bg" />
          <span aria-hidden="true">{seconds === null ? "Reconnecting…" : `Reconnecting… retry in ${seconds}s`}</span>
          <span className="sr-only">Reconnecting to live updates</span>
        </span>
      );
    }
    case "offline":
      return (
        <span role="status" data-slot="live-indicator" data-status="offline" className={cn(PILL, "border-dashed border-input text-muted-foreground")}>
          <WifiOff aria-hidden="true" data-shape="icon" className="size-3" />
          <span aria-hidden="true">{view.dataAsOf === null ? "Offline" : `Offline · data from ${formatClock(view.dataAsOf)}`}</span>
          <span className="sr-only">Live updates are paused. The page shows the last data it received.</span>
        </span>
      );
    case "stopped":
      return (
        <span role="status" data-slot="live-indicator" data-status="stopped" className={cn(PILL, "border-dashed border-input text-muted-foreground")}>
          <WifiOff aria-hidden="true" data-shape="icon" className="size-3" />
          <span aria-hidden="true">Live updates stopped</span>
          <span className="sr-only">Live updates have stopped</span>
        </span>
      );
  }
}

/** The pill, connected to the live stream. */
export function LiveIndicator() {
  const view = useLiveStatus();
  const counting = view.status === "reconnecting";
  const now = useNow(counting);
  return <LiveIndicatorView view={view} now={now} />;
}
