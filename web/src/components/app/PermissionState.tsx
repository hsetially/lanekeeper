import type { ReactNode } from "react";
import type { Role } from "@/api/generated/model";
import { roleLabel } from "@/auth/roles";
import { Lock } from "@/icons";
import { cn } from "@/lib/utils";

/**
 * The design's permission state (States index, Permission): a lock, "<Screen> needs the <Role> role", the role the person has,
 * and who to ask. A permission is a screen, never a redirect, and the server enforces it whatever this shows (S4). Disabled
 * controls inside a screen are a different thing: they stay visible with a tooltip (`GatedButton`).
 *
 * The design names the admin to ask ("Ask Priya Shah"). The contract has no way for a non-admin to name admins (CG14), so the
 * text says "an Admin" (design/DEVIATIONS.md).
 */
export function PermissionState({
  what,
  need,
  have,
  action,
  as: Heading = "h2",
  className,
}: {
  /** The screen or action that is out of reach: "Approvals", "Admin". */
  what: string;
  /** The role it needs. `null` for a person who has no role at all. */
  need: Role | null;
  have: Role | null;
  /** The one next step the person can take, such as Request a role change. */
  action?: ReactNode;
  as?: "h1" | "h2";
  className?: string;
}) {
  const noRole = need === null || have === null;
  return (
    <div data-slot="permission-state" className={cn("flex flex-col items-center gap-2 px-6 py-10 text-center", className)}>
      <Lock aria-hidden="true" className="size-5 text-status-locked-fg" />
      <Heading className="text-base font-semibold">{noRole ? "You do not have access" : `${what} needs the ${roleLabel(need)} role`}</Heading>
      <p className="max-w-sm text-sm text-muted-foreground">
        {noRole ? "You have no role yet. Ask an Admin to grant access." : `You are signed in as ${roleLabel(have)}. Ask an Admin to change your role.`}
      </p>
      {action ? <div className="mt-1 flex flex-wrap items-center justify-center gap-2">{action}</div> : null}
    </div>
  );
}
