import type { ReactElement } from "react";
import type { PickupState } from "@/api/generated/model";
import { StatusBadge } from "@/components/app/StatusBadge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { pickupView, type PickupSubject } from "./model";

// The chip of one pickup state (D85): Live, Live by a time (the client cache), Needs notify or restart, Needs config-server
// restart. It is the shared StatusBadge with the pickup words around it, so the icon, the colour and the label are the badge's
// and the mapping from the contract's states is `pickupVariant`, written once. The chip adds what the badge cannot:
//  - the time, read honestly (a time that is missing, unreadable or past is said so, never made up; see model.ts);
//  - a tooltip that says in plain words when the change takes effect. It is also in the page for a screen reader (the full chip
//    carries it as visually hidden text), because the chip is not a tab stop and a tooltip that opens only on hover is not a way
//    to read it;
//  - a compact size for a table cell or a tree row: the icon and a count, with the words kept for assistive technology and the
//    tooltip. It has no native `title`, so the words are not drawn twice.
//
// A state the hub did not give (null) draws nothing: "no state" is not "Live". A state this app does not know draws "Pickup
// unknown" and never repeats the value. Nothing from the hub is drawn as text here except through the model, which only
// produces fixed words and numbers; a caller that adds `details` or `words` passes its own fixed words and numbers, and any
// text of the hub's (a path, a name) goes through SafeText beside the chip.

export type PickupChipProps = {
  /** `ServiceState.pickup` or `FileEntry.pickup_state`. Null or absent: the hub did not say, so nothing is drawn. */
  state: PickupState | null | undefined;
  /** What the chip stands for: one service (the default), one file, or `count` services in the same state. */
  subject?: PickupSubject;
  /** `ServiceState.live_by`. Leave it out where the data has none (a draft's list of services); null is "the hub gave none". */
  liveBy?: number | null | undefined;
  /** The page's clock (`useNow`), to tell a time to come from a time that has passed. */
  now?: number | undefined;
  /** A number of services, drawn beside the icon. */
  count?: number | undefined;
  density?: "full" | "compact";
  /** Draw nothing for Live, for a list where only what is waiting is worth a mark. */
  hideLive?: boolean;
  /** More sentences for the tooltip, after the first: the cache lifetime, the notifications flag. */
  details?: readonly string[] | undefined;
  /** The first sentence of the tooltip when the caller knows better (a folder says how many of its loaded files wait). */
  words?: string | undefined;
  className?: string | undefined;
};

const stop = (sentence: string): string => (sentence.endsWith(".") ? sentence : `${sentence}.`);

export function PickupChip({ state, subject = "service", liveBy, now, count, density = "full", hideLive = false, details = [], words, className }: PickupChipProps): ReactElement | null {
  if (state === null || state === undefined) return null;
  const view = pickupView(state, { subject, liveBy, now, count });
  if (hideLive && view.state === "live") return null;
  // A sentence is said once, whoever gave it (a caller's line may repeat the first), and each is its own line in the tooltip.
  const sentences = [...new Set([words ?? view.words, ...details].map(stop))];
  const tip = (
    <TooltipContent>
      <span className="flex flex-col gap-0.5">
        {sentences.map((sentence) => (
          <span key={sentence}>{sentence}</span>
        ))}
      </span>
    </TooltipContent>
  );
  const badge = (
    <StatusBadge
      variant={view.variant}
      {...(view.label === undefined ? {} : { label: view.label })}
      {...(view.detail === undefined ? {} : { detail: view.detail })}
      {...(count === undefined ? {} : { count })}
      className={cn("tabular-nums", density === "compact" && "[&>span:last-child]:sr-only")}
    />
  );

  if (density === "compact") {
    // The name is the count and the words, as the badge's compact form names itself; the label inside is out of sight and out of
    // the accessibility tree (an image has no children to read), so the tooltip is the only place the sentence is drawn.
    const name = count === undefined ? view.name : `${count} ${view.name}`;
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span data-slot="pickup-chip" data-pickup={view.state} data-density="compact" role="img" aria-label={name} className={cn("inline-flex max-w-full", className)}>
            {badge}
          </span>
        </TooltipTrigger>
        {tip}
      </Tooltip>
    );
  }
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span data-slot="pickup-chip" data-pickup={view.state} data-density="full" className={cn("inline-flex max-w-full min-w-0 items-center gap-1.5", className)}>
          {badge}
          {view.qualifier === null ? null : <span className="min-w-0 truncate text-xs tabular-nums text-muted-foreground">{view.qualifier}</span>}
          <span className="sr-only">{sentences.join(" ")}</span>
        </span>
      </TooltipTrigger>
      {tip}
    </Tooltip>
  );
}
