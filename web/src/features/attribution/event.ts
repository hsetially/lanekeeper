import type { AuditEvent } from "@/api/generated/model";
import { describeAttribution, userLabel, type WhoContext } from "@/components/attribution/model";

// Who made an audit event: the User column of the audit log and the line under a row of the activity feed.
//
// The audit read carries the event's actor as an id (who recorded it: for an approval, the approver) and, for a change to a file,
// an attribution that names who really made the change. The attribution wins. Where it names nobody, the event says "unknown
// author" rather than naming whoever recorded it, except for a tool write (Lanekeeper wrote it, so its recorder is its author).
// Nothing is guessed: a person is named only by the attribution, or by the event's own actor, and a user the screen has no name
// for is "Lanekeeper user" and the start of their id.

export type EventWhoView =
  | { readonly kind: "named"; /** Untrusted text (a name, a login, a job name): draw it with SafeText. */ readonly label: string; /** The server is not sure (low or medium). */ readonly probable: boolean }
  | { readonly kind: "unknown" }
  | { readonly kind: "system" };

export function eventWho(event: Pick<AuditEvent, "actor" | "attribution">, context: WhoContext = {}): EventWhoView {
  const attribution = event.attribution;
  if (attribution !== null) {
    if (attribution.source === "unknown" || (attribution.actor === null && attribution.source !== "tool_write")) return { kind: "unknown" };
    if (attribution.actor !== null) {
      return { kind: "named", label: describeAttribution(attribution, context).who.label, probable: attribution.confidence === "low" || attribution.confidence === "medium" };
    }
  }
  return event.actor === null ? { kind: "system" } : { kind: "named", label: userLabel(event.actor, context), probable: false };
}
