import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { AppLink } from "@/routes/AppLink";

/**
 * The `action` of a banner that says an agent is offline: a link to the Admin page where the agents are, for someone who may open
 * it, and nothing for anyone else. Spread it into the banner's props: `<Banner {...checkAgentAction(canCheck)}>`.
 */
export function checkAgentAction(canCheck: boolean): { action?: ReactNode } {
  if (!canCheck) return {};
  return {
    action: (
      <Button asChild variant="secondary" size="sm">
        <AppLink route="admin-swimlanes">Check agent</AppLink>
      </Button>
    ),
  };
}
