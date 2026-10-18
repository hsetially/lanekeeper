import { PickupState, type ServiceState } from "@/api/generated/model";
import type { StatusVariant } from "@/components/app/StatusBadge";

// The pickup of a service (D85): when a change takes effect for it. There are four states and no bare "pending restart". The
// pickup CHIPS are task T6's; this file only maps the contract's enum to the app's existing status vocabulary and summarises
// the counts the swimlane list carries, so that the screens of T4 say the same thing T6 will.

export function pickupVariant(state: PickupState): StatusVariant {
  switch (state) {
    case PickupState.live:
      return "live";
    case PickupState.live_within_ttl:
      return "live-by";
    case PickupState.needs_notify_or_restart:
      return "needs-notify-or-restart";
    case PickupState.needs_config_server_restart:
      return "needs-config-server-restart";
    default:
      return "needs-notify-or-restart";
  }
}

export type PickupSummary = {
  /** Services that need a person to notify or restart something. */
  readonly actionable: number;
  /** The more urgent of the two states that need action, or null when none does. */
  readonly variant: "needs-config-server-restart" | "needs-notify-or-restart" | null;
  /** Services that will pick the change up by themselves when their cache expires. */
  readonly withinTtl: number;
  /** The words for a tooltip. */
  readonly title: string;
};

const whole = (n: number): number => (Number.isInteger(n) && n > 0 ? n : 0);

/** The Pickup column: the pairs of `SwimlaneSummary.pickup_counts` (non-live states, each once) as one count and one colour. */
export function pickupSummary(counts: readonly (readonly [PickupState, number])[] | undefined): PickupSummary {
  let config = 0;
  let notify = 0;
  let ttl = 0;
  for (const [state, n] of counts ?? []) {
    if (state === PickupState.needs_config_server_restart) config += whole(n);
    else if (state === PickupState.needs_notify_or_restart) notify += whole(n);
    else if (state === PickupState.live_within_ttl) ttl += whole(n);
  }
  const parts: string[] = [];
  if (config > 0) parts.push(`${config} ${config === 1 ? "needs" : "need"} config-server restart`);
  if (notify > 0) parts.push(`${notify} ${notify === 1 ? "needs" : "need"} notify or restart`);
  if (ttl > 0) parts.push(`${ttl} ${ttl === 1 ? "lives within its" : "live within their"} cache time`);
  return {
    actionable: config + notify,
    variant: config > 0 ? "needs-config-server-restart" : notify > 0 ? "needs-notify-or-restart" : null,
    withinTtl: ttl,
    title: parts.length === 0 ? "Every service has picked up its config" : parts.join(", "),
  };
}

const baseName = (path: string): string => path.slice(path.lastIndexOf("/") + 1);

/** The files a service is waiting on: the first one's file name, how many more there are, and all of them for a tooltip. */
export function pendingFilesSummary(service: ServiceState): { first: string | null; more: number; all: string[] } {
  const all = service.pending_files;
  const head = all[0];
  return { first: head === undefined ? null : baseName(head), more: Math.max(0, all.length - 1), all: [...all] };
}
