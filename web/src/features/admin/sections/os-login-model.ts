import type { User, UserStatus } from "@/api/generated/model";
import { visibleText } from "@/components/app/safe-text";

// The OS Login mapping: the login name that auditd records for an edit on an NFS VM (`ada_example_com`, Google's form of
// ada@example.com) and the Lanekeeper user it stands for. A row here is the admin's own mapping, and it is looked up first, by exact
// name (the registry's `attribution::os_login`); a name with no row is matched to the one active user whose email has that form, and
// otherwise stands alone. The registry does not ask the status of a mapped user: the record is history, and a user who has since been
// disabled still made the change.

/** The contract's `ShortText`. */
export const MAX_OS_LOGIN = 256;

// What an OS Login name is made of. A name is also a segment of the address that is written to (`/os-login-users/{login}`), where the
// generated client puts it as it is, so a character that is not one of these (a slash, a dot-dot, a question mark, a percent) must never
// get that far, whatever the hub would have done with it.
const NAME = /^[A-Za-z0-9_][A-Za-z0-9._@+-]*$/;
const NAME_HELP = "Use the name as the VM records it: letters, digits and . _ - @ +, starting with a letter, a digit or _";

/** The message to show beside the field, or null when the name can be sent. It never repeats the value. */
export function checkOsLogin(value: string): string | null {
  if (value === "") return "Enter the OS Login username";
  if (value.length > MAX_OS_LOGIN) return `At most ${MAX_OS_LOGIN} characters`;
  return NAME.test(value) ? null : NAME_HELP;
}

/**
 * The name as one segment of an address. The generated client writes a path parameter as it is, so a name that came from the hub (a
 * row to delete) is encoded here and cannot reach another address; the hub decodes it.
 */
export const pathSegment = (name: string): string => encodeURIComponent(name);

export function indexUsers(users: readonly User[]): ReadonlyMap<string, User> {
  return new Map(users.map((user) => [user.id, user]));
}

/** What to say beside a user who is not active, or null. */
export function userStatusNote(status: UserStatus): string | null {
  if (status === "disabled") return "disabled";
  if (status === "pending") return "no role yet";
  return null;
}

/** A user as the text of an `<option>`, which cannot hold markup: hidden characters are written out. */
export function userOptionText(user: User): string {
  const note = userStatusNote(user.status);
  return `${visibleText(user.display_name)} · ${visibleText(user.email)}${note === null ? "" : ` (${note})`}`;
}
