import { useMemo } from "react";
import type { User } from "@/api/generated/model";
import { adminQueries } from "@/features/admin/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";

// Who is who, for the two Admin-only screens (approvals and audit). The proposals and the audit events carry a user as an id and
// no name (`UserId`, `<tenant>:<object>`), and the one list of users is Admin-only, which these screens are too. A person that is
// not in the list (removed, or the list failed to load) is shown by the start of their id, so a row is never nameless and never
// blank. A name is the person's own text: the screens render it through SafeText.

const PAGE = 500;

/** `user 1a2b3c4d`: the start of the object part of the id. */
export function fallbackName(id: string): string {
  const object = id.includes(":") ? id.slice(id.indexOf(":") + 1) : id;
  return `user ${object.slice(0, 8)}`;
}

/** The first word of a name: the Decision column says "Approved · Priya". */
export function firstName(name: string): string {
  const first = name.trim().split(/\s+/)[0];
  return first === undefined || first === "" ? name : first;
}

export type People = {
  readonly loaded: boolean;
  /** The user, if the list has them. */
  readonly user: (id: string | null | undefined) => User | undefined;
  /** What to call them. */
  readonly name: (id: string | null | undefined) => string;
  /** The other active Admins: who can decide when the signed-in person may not. */
  readonly otherAdmins: (me: string | null) => readonly User[];
  /** Every user, for a filter. */
  readonly all: readonly User[];
};

export function usePeople(): People {
  const pages = useCursorPages({ ...adminQueries.users({ limit: PAGE }), autoLoad: true });
  const { items, status } = pages;
  return useMemo<People>(() => {
    const byId = new Map<string, User>(items.map((u) => [u.id, u]));
    const user = (id: string | null | undefined): User | undefined => (id === null || id === undefined ? undefined : byId.get(id));
    return {
      loaded: status === "success",
      user,
      name: (id) => (id === null || id === undefined ? "Lanekeeper" : (user(id)?.display_name ?? fallbackName(id))),
      otherAdmins: (me) => items.filter((u) => u.role === "admin" && u.status === "active" && u.id !== me),
      all: items,
    };
  }, [items, status]);
}
