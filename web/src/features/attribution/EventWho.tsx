import type { AuditEvent } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import type { NameLookup } from "@/components/attribution/model";
import { UnknownAuthorBadge } from "@/components/attribution/UnknownAuthorBadge";
import { cn } from "@/lib/utils";
import { eventWho } from "./event";

// Who made an audit event, for a User column or the line under a row of the activity feed. See `eventWho` for how it is decided.
// It does not read the session: a log reads best with names, so "You" is only said when the screen passes `you`.

export type EventWhoProps = {
  event: Pick<AuditEvent, "actor" | "attribution">;
  /** The signed-in person's id: their events are "You". Without it nobody is. */
  you?: string | null | undefined;
  /** The user list, if the screen has it (the Admin screens do). Without it a user is "Lanekeeper user" and a short id. */
  nameOf?: NameLookup | undefined;
  className?: string;
};

export function EventWho({ event, you, nameOf, className }: EventWhoProps) {
  const who = eventWho(event, { you, nameOf });
  switch (who.kind) {
    case "unknown":
      return <UnknownAuthorBadge className={className} />;
    case "system":
      return (
        <span data-slot="event-who" data-who="system" className={cn("text-muted-foreground", className)}>
          Lanekeeper
        </span>
      );
    case "named":
      return (
        <span data-slot="event-who" data-who="named" className={cn("inline-flex min-w-0 max-w-full items-baseline gap-1.5", className)}>
          <b className="min-w-0 truncate font-medium">
            <SafeText text={who.label} />
          </b>
          {who.probable ? <small className="text-xs text-muted-foreground">probable</small> : null}
        </span>
      );
  }
}
