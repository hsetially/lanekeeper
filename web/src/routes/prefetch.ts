import { useEffect } from "react";
import { useRouter } from "@tanstack/react-router";
import type { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import type { AppRouter } from "@/router";
import { ROUTES, type RouteDef } from "./table";

// Idle prefetch (P10: a route change with its data cached takes under 100 ms). The sidebar's destinations are the screens a
// person opens next, so while the browser is idle each one's chunk is loaded, one at a time, and the click that follows finds
// it already there. This loads the chunk (an `import()` of app code, which only defines components), unlike the editor's
// warm-up, which only fetches and never evaluates: Monaco is megabytes, and evaluating it installs the global patches.
//
// Only what the role may open is loaded (a Viewer never fetches the Admin chunks), nothing is loaded when the person asked
// the browser to save data, and a destination that cannot be loaded is skipped: the route will try again when it is used.

/** The route ids of the sidebar, in its order (the Engineering Handoff's `Sidebar` row). */
export const SIDEBAR_DESTINATIONS = ["swimlanes", "compare", "status", "findings", "approvals", "audit", "admin-users"] as const;

/** The destinations a role may open. A person with no role may open none. */
export function prefetchTargets(role: Role | null): RouteDef[] {
  return SIDEBAR_DESTINATIONS.flatMap((id) => {
    const def = ROUTES.find((r) => r.id === id);
    if (!def || def.access === "public" || def.path.includes(":") || !roleAtLeast(role, def.access)) return [];
    return [def];
  });
}

type Preloader = { preloadRoute: (options: { to: string }) => Promise<unknown> };
type Options = {
  /** Run `run` when the browser is idle; returns the function that cancels it. */
  schedule?: (run: () => void) => () => void;
  saveData?: () => boolean;
};

const IDLE_TIMEOUT_MS = 5000;

function scheduleIdle(run: () => void): () => void {
  if (typeof requestIdleCallback === "function") {
    const handle = requestIdleCallback(run, { timeout: IDLE_TIMEOUT_MS });
    return () => {
      if (typeof cancelIdleCallback === "function") cancelIdleCallback(handle);
    };
  }
  const handle = setTimeout(run, 200);
  return () => clearTimeout(handle);
}

function saveDataOn(): boolean {
  const connection = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection;
  return connection?.saveData === true;
}

/** Load the destinations one per idle moment. Returns the function that stops it. */
export function idlePreload(router: Preloader, targets: readonly RouteDef[], { schedule = scheduleIdle, saveData = saveDataOn }: Options = {}): () => void {
  if (saveData() || targets.length === 0) return () => undefined;
  let cancelled = false;
  let cancelPending: (() => void) | undefined;
  const step = (index: number): void => {
    const target = targets[index];
    if (cancelled || !target) return;
    cancelPending = schedule(() => {
      if (cancelled) return;
      // A failure here is not the person's business: the route asks again when it is used.
      void router.preloadRoute({ to: target.path }).catch(() => undefined);
      step(index + 1);
    });
  };
  step(0);
  return () => {
    cancelled = true;
    cancelPending?.();
  };
}

/** Mount once, below the session provider: loads the sidebar's destinations for the signed-in role while the page is idle. */
export function useIdlePrefetch(): void {
  const router = useRouter<AppRouter>();
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  const ready = session.status === "ready";
  useEffect(() => {
    if (!ready) return undefined;
    return idlePreload(router, prefetchTargets(role));
  }, [router, ready, role]);
}
