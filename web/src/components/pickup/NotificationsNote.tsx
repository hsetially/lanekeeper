import type { ReactElement } from "react";
import { Bell } from "@/icons";
import { cn } from "@/lib/utils";
import { notificationsText } from "./model";

// `ServiceState.notifications_enabled`, as the hub reported it. Whether the services of a swimlane subscribe to change
// notifications is an open question (Q35), so this note says "on" or "off" and that the hub said so, and nothing about what the
// flag does for the service. If the answer to Q35 changes what the screens should say, it changes the words in model.ts
// (`notificationsText`), and nowhere else.

export type NotificationsNoteProps = {
  /** `ServiceState.notifications_enabled`. Anything that is not a boolean is "not reported by the hub". */
  enabled: boolean | null | undefined;
  className?: string | undefined;
};

export function NotificationsNote({ enabled, className }: NotificationsNoteProps): ReactElement {
  const state = enabled === true ? "on" : enabled === false ? "off" : "unreported";
  return (
    <span data-slot="notifications-note" data-notifications={state} className={cn("inline-flex min-w-0 items-center gap-1 text-xs text-muted-foreground", className)}>
      <Bell aria-hidden="true" className="size-3 shrink-0" />
      <span className="min-w-0 truncate">{notificationsText(enabled)}</span>
    </span>
  );
}
