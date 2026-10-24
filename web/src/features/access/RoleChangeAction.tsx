import type { Role } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { RequestRoleButton } from "./RequestRoleDialog";

/**
 * The permission state's one next step, ready to mount: `<PermissionState ... action={<RoleChangeAction need={Role.admin} />} />`.
 * It reads the signed-in person's role itself, so a screen that shows a permission state does not have to pass it, and it draws
 * nothing for a person who cannot ask (no role yet, or already an Admin).
 *
 * A default export on purpose: the shell's permission screen is in the first chunk of the app and must not carry the dialog, so it
 * (and any other screen) loads this with `lazy(() => import("@/features/access/RoleChangeAction"))`.
 */
export default function RoleChangeAction({ need }: { /** The role the screen asked for. */ need?: Role | null }) {
  const session = useSession();
  const have = session.status === "ready" ? session.role : null;
  return <RequestRoleButton have={have} {...(need ? { need } : {})} />;
}
