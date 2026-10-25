import { useMemo } from "react";
import { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import type { NameLookup } from "@/components/attribution/model";
import { adminQueries } from "@/features/admin/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";

// Who is who, for the screens that draw a person from an id. The contract names a user by id only (`UserId`,
// `<Entra tenant>:<object>`), and the one list of names is the Admin-only user list. So:
//   - an Admin's screens get the names, from the list (the same cache entry as the approvals and audit screens read);
//   - everyone else's get none, and the words say "Lanekeeper user" and a short id (src/components/attribution/model.ts);
//   - a list that fails, or a person who is not in it, leaves a name missing. A name is never made up.
// A name is the person's own text: the components draw it through SafeText.

const PAGE = 500;

export type ActorNames = {
  /** The signed-in person's id, or null before the session is there. */
  readonly you: string | null;
  readonly nameOf: NameLookup;
  /** The names that can be known are known: always true for a person who may not read the list. */
  readonly loaded: boolean;
};

export function useActorNames(): ActorNames {
  const session = useSession();
  const ready = session.status === "ready";
  const isAdmin = ready && roleAtLeast(session.role, Role.admin);
  const you = ready ? session.me.id : null;
  const { items, status } = useCursorPages({ ...adminQueries.users({ limit: PAGE }), autoLoad: true, enabled: isAdmin });
  return useMemo<ActorNames>(() => {
    const byId = new Map<string, string>(items.map((u) => [u.id, u.display_name]));
    return { you, nameOf: (id) => byId.get(id), loaded: !isAdmin || status !== "pending" };
  }, [you, items, isAdmin, status]);
}
