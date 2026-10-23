import type { ReactElement } from "react";
import type { ServiceState } from "@/api/generated/model";
import { formatTtl, isShownTime, needsAction, notificationsText, PickupChip } from "@/components/pickup";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { PickupState } from "@/api/generated/model";
import { formatRelative } from "@/lib/time";
import { pendingFilesSummary } from "@/features/swimlanes/pickup";

// The "Config status" cell of a service on the Services tab (D85): whether the service has picked up its config, since when it has
// been waiting for a person, and which file it waits on. The tooltip of the chip adds the cache lifetime (for a resource file that
// waits on the cache) and the notifications flag, as the hub reported them (Q35 is open: nothing here says what the flag does).
//
// The row is 52 px high with room for two lines, so everything past those two lines is in the tooltip.

/** The files named in the native title of the "Waiting on" line. A service may wait on 500; a title of 500 paths is not a title. */
const TITLE_FILES = 20;

export function ServicePickup({ service, now }: { service: ServiceState; now: number }): ReactElement {
  const files = pendingFilesSummary(service);
  const since = needsAction(service.pickup) && isShownTime(service.pending_since) ? service.pending_since : null;
  const details: string[] = [];
  const lifetime = service.pickup === PickupState.live_within_ttl ? formatTtl(service.cache_ttl_secs) : null;
  if (lifetime !== null) details.push(`Client cache lifetime: ${lifetime}`);
  details.push(notificationsText(service.notifications_enabled));
  const listed = files.all.slice(0, TITLE_FILES);
  // The paths are the hub's text: in a title attribute each goes through `visibleText`, which makes a bidi override or a zero-width
  // character a visible marker (a title cannot hold the SafeText element). Each one, not the joined text: the line break between
  // two paths is ours, and `visibleText` would mark it.
  const title = listed.map(visibleText).join("\n") + (files.all.length > TITLE_FILES ? `\nand ${files.all.length - TITLE_FILES} more` : "");
  return (
    <span className="flex min-w-0 flex-col gap-0.5">
      <span className="flex min-w-0 items-center gap-2">
        <PickupChip state={service.pickup} subject="service" liveBy={service.live_by} now={now} details={details} />
        {since === null ? null : (
          <span title={`since ${formatRelative(since, now)}`} className="min-w-0 truncate text-xs text-muted-foreground">
            since {formatRelative(since, now)}
          </span>
        )}
      </span>
      {files.first !== null ? (
        <span title={title} className="truncate text-xs text-muted-foreground">
          Waiting on <SafeText text={files.first} className="font-mono" />
          {files.more > 0 ? ` and ${files.more} more` : ""}
        </span>
      ) : null}
    </span>
  );
}
