import { StatusBadge } from "@/components/app/StatusBadge";
import type { PageProps } from "@/routes/props";
import { AccessFrame, AccessHeading, SignOutLink } from "./AccessFrame";

// The Access Screen's "Access requested" (frames 1c and 1d), at the path the hub sends a user with no role to
// (/access-requested, routes/auth.rs). A public route: the hub answers this user 403 and names nobody, so the design's
// "Signed in as" and "Request sent" rows have no data to show and are not drawn (design/DEVIATIONS.md, CG14).
export default function PendingPage(_props: PageProps<"access-pending">) {
  return (
    <AccessFrame>
      <AccessHeading badge={<StatusBadge variant="approval" label="Pending approval" />} title="Access requested">
        An admin must approve your access before you can use Lanekeeper. You'll be able to sign in as soon as they do.
      </AccessHeading>
      <SignOutLink />
    </AccessFrame>
  );
}
