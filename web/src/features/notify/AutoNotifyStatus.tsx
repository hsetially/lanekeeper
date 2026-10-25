import { Role, type SwimlaneDetail } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { Badge } from "@/components/ui/badge";
import { Ban, Bell } from "@/icons";
import { AppLink } from "@/routes/AppLink";

// Whether a swimlane tells the config-server by itself which paths changed after a write ("auto-notify", off by default). A
// read-only line for the Services tab: the answer is `SwimlaneDetail.auto_notify`, which anyone who can see the swimlane can read, so
// an Operator sees it without the Admin-only `adminGetAutoNotify`. The switch is the Admin page's (AutoNotifySwitch, the one
// optimistic write of the app): this is not a second writer, it only links there for an Admin. It tells the on state from the off
// state by its words and its icon, not by colour alone.

export function AutoNotifyStatus({ lane }: { lane: Pick<SwimlaneDetail, "auto_notify"> }) {
  const session = useSession();
  const isAdmin = roleAtLeast(session.status === "ready" ? session.role : null, Role.admin);
  const on = lane.auto_notify;
  const Icon = on ? Bell : Ban;

  return (
    <div role="group" aria-label="Auto-notify" className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
      <Badge variant={on ? "outline" : "muted"}>
        <Icon aria-hidden="true" />
        {on ? "Auto-notify on" : "Auto-notify off"}
      </Badge>
      <span>{on ? "After each write that is applied, the agent tells the config-server which paths changed." : "Nobody is notified automatically. Use Notify services after a write."}</span>
      {isAdmin ? (
        <AppLink route="admin-swimlanes" className="font-medium text-foreground underline underline-offset-2">
          Change in Admin
        </AppLink>
      ) : (
        <span>Only an Admin can change this.</span>
      )}
    </div>
  );
}
