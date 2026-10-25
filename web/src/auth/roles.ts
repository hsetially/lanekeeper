import { Role } from "@/api/generated/model";

/** viewer < editor < operator < admin: the order of the contract's Role enum. */
export const ROLE_ORDER: readonly Role[] = Object.values(Role);

export function roleAtLeast(have: Role | null | undefined, need: Role): boolean {
  if (have === null || have === undefined) return false;
  return ROLE_ORDER.indexOf(have) >= ROLE_ORDER.indexOf(need);
}

export const roleLabel = (role: Role): string => role.charAt(0).toUpperCase() + role.slice(1);
