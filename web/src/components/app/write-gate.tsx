import { createContext, useContext, useMemo, type ReactNode } from "react";

// Who may press a write button, and why not. The server enforces permissions regardless; this only explains.
// The role order and the current role come from the session (T1 mounts the provider from the generated Role
// values and the live connection state), so nothing here spells out the API's role list.

export type WriteGateValue = {
  /** Role names from lowest to highest. */
  roles: readonly string[];
  /** The signed-in user's role, or null when they have none yet. */
  role: string | null;
  /** False while live updates are paused: writes are disabled, reads stay open. */
  online: boolean;
};

export type GateResult = { allowed: boolean; reason: string | null };

const WriteGateContext = createContext<WriteGateValue | null>(null);

export function WriteGateProvider({ value, children }: { value: WriteGateValue; children: ReactNode }) {
  const { roles, role, online } = value;
  const stable = useMemo<WriteGateValue>(() => ({ roles, role, online }), [roles, role, online]);
  return <WriteGateContext.Provider value={stable}>{children}</WriteGateContext.Provider>;
}

const capitalise = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

export const OFFLINE_REASON = "Unavailable offline";

/**
 * Whether an action is allowed now. `minRole` is the lowest role that may do it. Fails closed: with no provider, or a
 * minimum role the session does not know, the action is not allowed. The permission reason comes before the
 * temporary offline one, because the user can fix only one of them by waiting.
 */
export function useWriteGate(minRole?: string, options: { needsConnection?: boolean } = {}): GateResult {
  const ctx = useContext(WriteGateContext);
  const needsConnection = options.needsConnection ?? true;
  if (!ctx) return { allowed: false, reason: "Not available until your permissions have loaded." };

  if (minRole !== undefined) {
    const need = ctx.roles.indexOf(minRole);
    if (need < 0) return { allowed: false, reason: "Not available." };
    const have = ctx.role === null ? -1 : ctx.roles.indexOf(ctx.role);
    if (ctx.role === null) return { allowed: false, reason: "You have no role yet. Ask an Admin to grant access." };
    if (have < need) return { allowed: false, reason: `Needs the ${capitalise(minRole)} role. Ask an Admin to grant it.` };
  }
  if (needsConnection && !ctx.online) return { allowed: false, reason: OFFLINE_REASON };
  return { allowed: true, reason: null };
}
