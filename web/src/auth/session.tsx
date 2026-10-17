import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { apiCaches } from "@/api/client";
import { ACCESS_DISABLED_PATH, ACCESS_PENDING_PATH, LOGIN_PATH } from "@/api/constants";
import { Role, type Me } from "@/api/generated/model";
import { useGetMe } from "@/api/generated/endpoints/me/me";
import { onSessionSignal } from "@/api/session-events";
import { ApiProblem } from "@/api/problem";
import { Banner } from "@/components/app/Banner";
import { CopyButton } from "@/components/app/CopyButton";
import { WriteGateProvider } from "@/components/app/write-gate";
import { Button } from "@/components/ui/button";
import { RouteLoading, ErrorScreen, PermissionScreen } from "@/routes/screens";
import type { RouteAccess } from "@/routes/table";
import type { AppRouter } from "@/router";
import { collectDrafts, hasDirty } from "./dirty";
import { validateNext } from "./params";
import { ROLE_ORDER, roleAtLeast } from "./roles";

// The session (S2, client side). It owns four things:
//  - who the user is: `getMe`, asked only on routes that need a session (a screen before sign-in makes no API call);
//  - what a 401 does: empties every cache that held the previous user's data, then goes to /signin?next=<this page>, unless
//    the page holds unsaved work, in which case it stays and says the session ended (the draft is not lost);
//  - what a user with no role yet or a disabled user does: go to the pages the hub itself redirects to;
//  - the role gate: a role too low shows a permission screen, never a redirect.

export type SessionState =
  | { status: "none" }
  | { status: "loading" }
  | { status: "error"; problem: ApiProblem }
  | { status: "ready"; me: Me; role: Role | null };

const SessionContext = createContext<SessionState>({ status: "none" });

export const useSession = (): SessionState => useContext(SessionContext);

/** The sign-in link for this page: the hub's own login endpoint with a validated return path, or no return path. */
export function loginHref(next: string | undefined): string {
  const valid = validateNext(next);
  return valid === undefined ? LOGIN_PATH : `${LOGIN_PATH}?return_to=${encodeURIComponent(valid)}`;
}

function SessionEndedAlert({ next }: { next: string | undefined }) {
  const draft = collectDrafts();
  return (
    <Banner
      variant="danger"
      title="Your session ended"
      action={
        <>
          {draft !== "" ? <CopyButton value={draft} what="draft" /> : null}
          <Button asChild>
            <a href={loginHref(next)}>Sign in again</a>
          </Button>
        </>
      }
    >
      Your work is still on this page. Copy it before you sign in again.
    </Banner>
  );
}

export function SessionProvider({ children, online = true }: { children: ReactNode; online?: boolean }) {
  const router = useRouter<AppRouter>();
  const queryClient = useQueryClient();
  const access = useRouterState({ select: (s) => s.matches.at(-1)?.staticData.access });
  const needsSession = access !== undefined && access !== "public";
  const [ended, setEnded] = useState<{ next: string | undefined } | null>(null);

  const me = useGetMe({ query: { enabled: needsSession, staleTime: 60_000 } });

  useEffect(
    () =>
      onSessionSignal((signal) => {
        const here = router.state.location.href;
        const publicPage = (router.state.matches.at(-1)?.staticData.access ?? "public") === "public";
        // Whatever happened, nothing the previous user saw may stay in memory.
        queryClient.clear();
        apiCaches.clear();
        if (signal.kind === "unauthenticated") {
          if (publicPage) return;
          if (hasDirty()) {
            setEnded({ next: validateNext(here) });
            return;
          }
          const next = validateNext(here);
          router.history.replace(next === undefined ? "/signin" : `/signin?next=${encodeURIComponent(next)}`);
        } else {
          router.history.replace(signal.kind === "access_pending" ? ACCESS_PENDING_PATH : ACCESS_DISABLED_PATH);
        }
      }),
    [router, queryClient],
  );

  const state = useMemo<SessionState>(() => {
    if (!needsSession) return { status: "none" };
    if (me.data) return { status: "ready", me: me.data, role: me.data.role ?? null };
    if (me.isError) return { status: "error", problem: me.error };
    return { status: "loading" };
  }, [needsSession, me.data, me.isError, me.error]);

  const roleName = state.status === "ready" ? state.role : null;
  return (
    <SessionContext.Provider value={state}>
      <WriteGateProvider value={{ roles: ROLE_ORDER, role: roleName, online }}>
        {ended ? <SessionEndedAlert next={ended.next} /> : null}
        {children}
      </WriteGateProvider>
    </SessionContext.Provider>
  );
}

/** Wraps a route's screen: waits for the session, then checks the role. Public routes pass straight through. */
export function RouteGate({ access, children }: { access: RouteAccess; children: ReactNode }) {
  const session = useSession();
  if (access === "public") return <>{children}</>;
  if (session.status === "none" || session.status === "loading") return <RouteLoading />;
  if (session.status === "error") {
    // A 401, or a user with no role yet, is being routed away by the session handler; show nothing in the meantime.
    if (session.problem.status === 401 || session.problem.kind === "access_pending" || session.problem.kind === "access_disabled") return <RouteLoading />;
    return <ErrorScreen error={session.problem} />;
  }
  if (!roleAtLeast(session.role, access)) return <PermissionScreen need={session.role === null ? null : access} />;
  return <>{children}</>;
}
