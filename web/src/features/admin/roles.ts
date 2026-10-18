import { Role, type User, type UserStatus } from "@/api/generated/model";
import { roleLabel } from "@/auth/roles";

// What each role is, in one sentence an Admin reads before giving it (the design's notes under "Approve as", and the decisions D35 to
// D37 and D48 for what a role may do). The order is the contract's: viewer < editor < operator < admin.

export const ROLE_CHOICES: readonly Role[] = Object.values(Role);

export function roleNote(role: Role): string {
  switch (role) {
    case Role.viewer:
      return "Viewer can read files, comparisons and findings but can't change anything.";
    case Role.editor:
      return "Editor can edit config files, and upload or delete them. If Requires approval is on for them, each change waits for an Admin.";
    case Role.operator:
      return "Operator can do everything an Editor can, and also restart and notify services.";
    case Role.admin:
      return "Admin can decide proposals, change roles and edit every swimlane, and can open this Admin area.";
    default:
      return "";
  }
}

/** `Role` as a word, and the lack of one as such. */
export const roleWord = (role: Role | null): string => (role === null ? "no role" : roleLabel(role));

/**
 * Whether a change would leave no ACTIVE admin among `users`. The people it is given must be all of them (the caller checks that the
 * list is complete and unfiltered); the hub checks regardless (409), this only says so before a dialog is opened for nothing.
 */
export function wouldLeaveNoAdmin(users: readonly User[], id: string, change: { readonly role?: Role | null; readonly status?: UserStatus }): boolean {
  const after = users.map((user) => (user.id === id ? { ...user, ...change } : user));
  return !after.some((user) => user.role === Role.admin && user.status === "active");
}

export function activeAdmins(users: readonly User[]): number {
  return users.filter((user) => user.role === Role.admin && user.status === "active").length;
}
