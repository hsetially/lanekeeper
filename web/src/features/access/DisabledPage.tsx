import { StatusBadge } from "@/components/app/StatusBadge";
import type { PageProps } from "@/routes/props";
import { AccessFrame, AccessHeading, SignOutLink } from "./AccessFrame";

// The Access Screen's "Your access is disabled" (frames 1e and 1f), at the path the hub sends a disabled user to
// (/access-disabled, routes/auth.rs). A public route that asks the hub for nothing: it cannot say when the account was turned off
// or which admin to contact (CG14), so it says "an admin" and the design's date and name are not drawn (design/DEVIATIONS.md).
export default function DisabledPage(_props: PageProps<"access-disabled">) {
  return (
    <AccessFrame>
      <AccessHeading badge={<StatusBadge variant="locked" label="Access disabled" />} title="Your access is disabled">
        An admin turned off your account. You can't use Lanekeeper until it is turned back on.
      </AccessHeading>
      <div className="flex flex-col gap-1 border-y border-border py-3">
        <span className="text-muted-foreground">To get access back, contact an admin.</span>
      </div>
      <SignOutLink />
    </AccessFrame>
  );
}
