import { useQueryClient } from "@tanstack/react-query";
import { createContext, useCallback, useContext, useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { useSession } from "@/auth/session";
import { ROLE_ORDER } from "@/auth/roles";
import { WriteGateProvider } from "@/components/app/write-gate";
import type { Notice } from "./invalidation";
import type { Live, LiveConfig } from "./live";
import type { LiveSnapshot, LiveStatus, StopReason } from "./sse-client";

// One live stream for the whole tab (D66, P13), mounted once at the router root, inside the session provider.
//
// When it is open: only for a signed-in user who has a role. A screen before sign-in, the pending and disabled pages and a
// user with no role yet never open it (the hub would answer 401 or 403 anyway). It is closed when the session goes (sign-out,
// a 401), and it is reconnected, not doubled, when the role changes: the hub filters what a user sees at connect time.
//
// It also decides `online` for the write gate (src/components/app/write-gate.tsx): writes are off while the stream is
// OFFLINE (three failed attempts in a row), never for a single dropped connection.

const LiveContext = createContext<Live | null>(null);

/** What the UI shows about the stream. `dataAsOf` is only set while the stream is down: while it is live it changes with every byte. */
export type LiveStatusView = {
  readonly status: LiveStatus;
  readonly failures: number;
  readonly retryAt: number | null;
  readonly dataAsOf: number | null;
  readonly stopReason: StopReason | null;
};

const IDLE_VIEW: LiveStatusView = { status: "idle", failures: 0, retryAt: null, dataAsOf: null, stopReason: null };

function view(snapshot: LiveSnapshot): LiveStatusView {
  return {
    status: snapshot.status,
    failures: snapshot.failures,
    retryAt: snapshot.retryAt,
    dataAsOf: snapshot.status === "live" ? null : snapshot.dataAsOf,
    stopReason: snapshot.stopReason,
  };
}

const sameView = (a: LiveStatusView, b: LiveStatusView): boolean =>
  a.status === b.status && a.failures === b.failures && a.retryAt === b.retryAt && a.dataAsOf === b.dataAsOf && a.stopReason === b.stopReason;

const noop = (): (() => void) => () => undefined;

/** The status of the stream, as a value that changes only when the status does (not on every event or heartbeat). */
export function useLiveStatus(): LiveStatusView {
  const live = useContext(LiveContext);
  const last = useRef<LiveStatusView>(IDLE_VIEW);
  const get = useCallback((): LiveStatusView => {
    if (!live) return IDLE_VIEW;
    const next = view(live.stream.getSnapshot());
    if (sameView(last.current, next)) return last.current;
    last.current = next;
    return next;
  }, [live]);
  return useSyncExternalStore(live ? live.stream.subscribe : noop, get, get);
}

/** What a component may ask of the stream. Without a provider these do nothing. */
export function useLiveActions(): { retryNow: () => void } {
  const live = useContext(LiveContext);
  return { retryNow: useCallback(() => live?.stream.retryNow(), [live]) };
}

/** Be told about a change that did not come through this app, or a proposal that changed (the toaster subscribes). */
export function useLiveNotices(handler: (notice: Notice) => void): void {
  const live = useContext(LiveContext);
  const latest = useRef(handler);
  useEffect(() => {
    latest.current = handler;
  });
  useEffect(() => live?.onNotice((notice) => latest.current(notice)), [live]);
}

/** The write gate: who the user is (the session) and whether the stream is up (the live layer). */
function ConnectedWriteGate({ children }: { children: ReactNode }) {
  const session = useSession();
  const { status } = useLiveStatus();
  const role = session.status === "ready" ? session.role : null;
  return <WriteGateProvider value={{ roles: ROLE_ORDER, role, online: status !== "offline" }}>{children}</WriteGateProvider>;
}

export function LiveProvider({ children, config }: { children: ReactNode; config?: LiveConfig | undefined }) {
  const queryClient = useQueryClient();
  const session = useSession();
  // The stream engine (the parser, the client, the plan of what an event invalidates) is a chunk of its own, loaded the first
  // time a signed-in user with a role is on screen: the sign-in screen and the pages before it never pay for it (P10).
  const [live, setLive] = useState<Live | null>(null);
  // The settings are read once, with the engine: a test passes a new object on every render, and a changed one must not restart the import.
  const [settings] = useState(config);

  const ready = session.status === "ready" && session.role !== null;
  const enabled = config?.enabled !== false && ready;
  const userId = session.status === "ready" ? session.me.id : null;
  const role = session.status === "ready" ? session.role : null;

  useEffect(() => {
    if (!enabled || live) return undefined;
    let cancelled = false;
    void import("./live").then(({ Live: Engine }) => {
      // Built once. React's development double effect (mount, unmount, mount) cancels the first import, so one engine exists.
      if (!cancelled) setLive((existing) => existing ?? new Engine(queryClient, settings));
    });
    return () => {
      cancelled = true;
    };
  }, [enabled, live, queryClient, settings]);

  // Who is signed in right now, for the effect that starts the stream (which must not restart when the user changes: the
  // effect below reconnects instead). Declared first, so it has run by the time the effects after it do.
  const latestUser = useRef<string | null>(null);
  useEffect(() => {
    latestUser.current = userId;
  });
  const startedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!enabled || !live) return undefined;
    // A different user than the one the stream last ran for (signed out, signed in as someone else): that user's resume
    // point means nothing to this one.
    if (startedFor.current !== null && startedFor.current !== latestUser.current) live.stream.resetResumePoint();
    startedFor.current = latestUser.current;
    live.start();
    return () => {
      live.stop();
    };
  }, [live, enabled]);

  // The role or the user changed while the stream is open: close it and open one at once, with the same resume point
  // (and none for a different user), instead of waiting up to 30 minutes for the hub to apply the new role.
  const before = useRef({ userId, role });
  useEffect(() => {
    const was = before.current;
    before.current = { userId, role };
    if (!enabled || !live) return;
    if (was.userId !== null && was.userId !== userId) {
      live.stream.resetResumePoint();
      live.stream.reconnect();
    } else if (was.role !== null && was.role !== role) {
      live.stream.reconnect();
    }
  }, [live, enabled, userId, role]);

  return (
    <LiveContext.Provider value={live}>
      <ConnectedWriteGate>{children}</ConnectedWriteGate>
    </LiveContext.Provider>
  );
}
