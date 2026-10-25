import { signalSessionProblem } from "@/api/client";
import { EVENTS_PATH } from "@/api/constants";
import type { DomainEvent } from "@/api/generated/model";
import { problemFromResponse, type ApiProblem } from "@/api/problem";
import { backoffDelayMs, MAX_FLOOR_MS } from "./backoff";
import { parseDomainEvent } from "./events";
import { SseParser, type SseItem } from "./sse-parser";

// The live stream (`GET /api/v1/events`, D66, P13): one long `fetch` read through a parser, and the state machine around it.
// `EventSource` is not used, for three reasons that matter here: it gives no control of the `Last-Event-ID` of a first
// connection, no control of the backoff (it retries by itself, without jitter), and no access to the status code of a failed
// connection (a 401 and a 503 look the same).
//
// What the hub guarantees, and what follows (crates/hub-platform/src/events/sse.rs; docs/interfaces.md, "Web additions"):
//  - only the `Last-Event-ID` header is read. A connection WITHOUT it is "fresh": no replay and no `resync`, and heartbeats
//    carry no id, so a client that has seen no id-bearing event has nothing to resume from. It must refetch instead;
//  - a connection WITH it replays the events after it (at most 1,000, else a `resync`), so a short drop costs nothing;
//  - a stream ends at 30 minutes (a final `retry:` frame, then a clean close), at shutdown, and when the session ends;
//  - at most 5 streams per user (a sixth is a 429), so the app holds exactly one, and closes it before it opens another.
//
// So the client refetches (`onResync`) in exactly three cases: on a connection that has no id to resume from, after it has
// been offline, and when the hub says `resync`. A routine rotation with an id refetches nothing.
//
// States: `connecting`, `live` (from the first frame), `reconnecting` (a failure, with the time of the next attempt),
// `offline` (three failures in a row: the UI says updates are paused and turns writes off; attempts go on at the cap), and
// `stopped` (401, 403 and 400: asking again will not help). `idle` is a stream nobody started.

export type LiveStatus = "idle" | "connecting" | "live" | "reconnecting" | "offline" | "stopped";
export type StopReason = "unauthorized" | "forbidden" | "bad_request";
/** Why the app must refetch what it shows: no id to resume from, back from offline, or the hub said so. */
export type ResyncReason = "no_id" | "was_offline" | "server";

export type LiveSnapshot = {
  readonly status: LiveStatus;
  /** Failed attempts in a row. Zero while live. */
  readonly failures: number;
  /** When the next attempt starts (ms since the epoch), while reconnecting or offline. */
  readonly retryAt: number | null;
  /** The last time a byte of a healthy stream arrived. Frozen while the stream is down: "showing data from 10:42". */
  readonly dataAsOf: number | null;
  readonly stopReason: StopReason | null;
  /** The id of the last event received: the resume point. */
  readonly lastEventId: string | null;
  /** How many connections were made. */
  readonly connections: number;
};

/** Three failed attempts in a row make the stream `offline`. */
export const OFFLINE_AFTER_FAILURES = 3;
/** No byte (a heartbeat comes every 15 s) for this long means the connection is dead even if nothing said so. */
export const STALL_MS = 45_000;
/** A stream that lived this long was healthy: its end starts the backoff over. */
export const STABLE_MS = 30_000;
/** What to wait after a clean end when the hub never said (its own `retry:` is 3000). */
export const DEFAULT_RETRY_HINT_MS = 3000;
const MAX_ID_DIGITS = 19;

type Handle = ReturnType<typeof setTimeout>;
type Timers = { setTimeout: (run: () => void, ms: number) => Handle; clearTimeout: (handle: Handle) => void };

export type LiveStreamOptions = {
  url?: string;
  fetch?: typeof fetch;
  now?: () => number;
  random?: () => number;
  timers?: Timers;
  stallMs?: number;
  /** An event the hub sent, validated. Never a `resync`: that is `onResync`. */
  onEvent: (event: DomainEvent) => void;
  onResync: (reason: ResyncReason) => void;
  /** A failed response, parsed. The default routes it as the fetch layer does: a 401 signs the user out of the app. */
  onProblem?: (problem: ApiProblem) => void;
  /** Where `online` is heard. The window, unless a test supplies its own. */
  onlineTarget?: EventTarget | undefined;
};

type End =
  | { kind: "superseded" }
  | { kind: "halt"; reason: StopReason }
  | { kind: "failed"; floorMs: number; opened: boolean }
  | { kind: "clean"; opened: boolean };

const IDLE: LiveSnapshot = { status: "idle", failures: 0, retryAt: null, dataAsOf: null, stopReason: null, lastEventId: null, connections: 0 };

const globalTimers: Timers = {
  setTimeout: (run, ms) => globalThis.setTimeout(run, ms),
  clearTimeout: (handle) => {
    globalThis.clearTimeout(handle);
  },
};

function isEventStream(response: Response): boolean {
  return (response.headers.get("content-type") ?? "").split(";")[0]?.trim().toLowerCase() === "text/event-stream";
}

/** The ids the hub sends are positive integers of at most 19 digits (`EventId::parse`). Anything else is never a resume point. */
function validId(id: string): boolean {
  return id.length <= MAX_ID_DIGITS && /^[1-9][0-9]*$/.test(id);
}

export class LiveStream {
  private snap: LiveSnapshot = IDLE;
  private readonly listeners = new Set<() => void>();
  private readonly fetchImpl: typeof fetch;
  private readonly now: () => number;
  private readonly random: () => number;
  private readonly timers: Timers;
  private readonly stallMs: number;
  private readonly url: string;
  private readonly onlineTarget: EventTarget | undefined;
  private readonly onProblem: (problem: ApiProblem) => void;

  private running = false;
  private generation = 0;
  private controller: AbortController | null = null;
  private retryTimer: Handle | null = null;
  private watchdog: Handle | null = null;
  private lastByteAt = 0;
  private backoffAttempt = 0;
  private serverRetryMs: number | null = null;
  private readonly onOnline = (): void => {
    this.retryNow();
  };

  constructor(private readonly options: LiveStreamOptions) {
    this.url = options.url ?? EVENTS_PATH;
    this.fetchImpl = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.now = options.now ?? (() => Date.now());
    this.random = options.random ?? Math.random;
    this.timers = options.timers ?? globalTimers;
    this.stallMs = options.stallMs ?? STALL_MS;
    this.onProblem = options.onProblem ?? signalSessionProblem;
    this.onlineTarget = "onlineTarget" in options ? options.onlineTarget : typeof window === "undefined" ? undefined : window;
  }

  // ---- what the UI reads ----

  getSnapshot = (): LiveSnapshot => this.snap;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private set(patch: Partial<LiveSnapshot>): void {
    this.snap = { ...this.snap, ...patch };
    for (const listener of [...this.listeners]) {
      try {
        listener();
      } catch {
        // A subscriber that throws must not stop the stream.
      }
    }
  }

  // ---- control ----

  /** Open the stream. Does nothing when it is already running: there is one stream. */
  start(): void {
    if (this.running) return;
    this.running = true;
    this.onlineTarget?.addEventListener("online", this.onOnline);
    this.set({ status: "connecting", failures: 0, retryAt: null, stopReason: null });
    void this.attempt();
  }

  /** Close the stream and everything that waits. The resume point is kept (see resetResumePoint). */
  stop(): void {
    this.running = false;
    this.cancelAttempt();
    this.onlineTarget?.removeEventListener("online", this.onOnline);
    this.set({ status: "idle", failures: 0, retryAt: null });
  }

  /** Try now instead of at the time the backoff chose. Does nothing unless the stream is waiting to retry. */
  retryNow(): void {
    if (!this.running) return;
    const status = this.snap.status;
    if (status !== "reconnecting" && status !== "offline") return;
    this.clearRetry();
    void this.attempt();
  }

  /** Close the open stream and open one at once with the same resume point: the role changed, so what the hub filters did. */
  reconnect(): void {
    if (!this.running) return;
    this.cancelAttempt();
    this.set({ status: "connecting", retryAt: null });
    void this.attempt();
  }

  /** Forget the id of the last event: the user changed, and another user's resume point means nothing. */
  resetResumePoint(): void {
    this.set({ lastEventId: null });
  }

  private cancelAttempt(): void {
    this.generation += 1;
    this.clearRetry();
    this.clearWatchdog();
    this.controller?.abort();
    this.controller = null;
  }

  private clearRetry(): void {
    if (this.retryTimer !== null) this.timers.clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }

  private clearWatchdog(): void {
    if (this.watchdog !== null) this.timers.clearTimeout(this.watchdog);
    this.watchdog = null;
  }

  // ---- one connection ----

  private async attempt(): Promise<void> {
    this.cancelAttempt();
    const gen = this.generation;
    const controller = new AbortController();
    this.controller = controller;
    const startedAt = this.now();
    this.lastByteAt = startedAt;
    const status = this.snap.status;
    this.set({ status: status === "reconnecting" || status === "offline" ? status : "connecting", retryAt: null, connections: this.snap.connections + 1 });
    this.armWatchdog(gen, controller);

    let end: End;
    try {
      end = await this.read(gen, controller);
    } catch {
      end = gen === this.generation ? { kind: "failed", floorMs: 0, opened: false } : { kind: "superseded" };
    } finally {
      // Whatever ended it, the request is over: a connection is never left open behind a new one.
      if (gen === this.generation) this.clearWatchdog();
      controller.abort();
    }
    if (gen !== this.generation) return;
    this.settle(end, startedAt);
  }

  private armWatchdog(gen: number, controller: AbortController): void {
    this.clearWatchdog();
    const wait = Math.max(0, this.lastByteAt + this.stallMs - this.now());
    this.watchdog = this.timers.setTimeout(() => {
      this.watchdog = null;
      if (gen !== this.generation) return;
      if (this.now() - this.lastByteAt >= this.stallMs) {
        controller.abort();
      } else {
        this.armWatchdog(gen, controller);
      }
    }, wait);
  }

  private async read(gen: number, controller: AbortController): Promise<End> {
    const headers: Record<string, string> = { accept: "text/event-stream" };
    const resume = this.snap.lastEventId;
    if (resume !== null) headers["last-event-id"] = resume;
    const response = await this.fetchImpl(this.url, {
      method: "GET",
      headers,
      cache: "no-store",
      credentials: "same-origin",
      mode: "cors",
      redirect: "error",
      referrerPolicy: "no-referrer",
      signal: controller.signal,
    });
    if (gen !== this.generation) return { kind: "superseded" };

    if (!response.ok) {
      const problem = await problemFromResponse(response);
      if (gen !== this.generation) return { kind: "superseded" };
      // The fetch layer's own handling of a failed response: a 401 empties the caches and signs out, a 403 for a user with
      // no role (or a disabled one) goes to its page. The same code runs for every other request of the app.
      this.onProblem(problem);
      if (problem.status === 401) return { kind: "halt", reason: "unauthorized" };
      if (problem.status === 403) return { kind: "halt", reason: "forbidden" };
      if (problem.status === 400) return { kind: "halt", reason: "bad_request" };
      return { kind: "failed", floorMs: (problem.retryAfterSeconds ?? 0) * 1000, opened: false };
    }
    const body = response.body;
    if (!isEventStream(response) || body === null) {
      // A 200 that is not the stream (a proxy's sign-in page) is not an answer to this request.
      void body?.cancel().catch(() => undefined);
      return { kind: "failed", floorMs: 0, opened: false };
    }

    const reader = body.getReader();
    const decoder = new TextDecoder();
    const parser = new SseParser();
    let opened = false;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (gen !== this.generation) return { kind: "superseded" };
        if (done) return { kind: "clean", opened };
        this.lastByteAt = this.now();
        let items: SseItem[];
        try {
          items = parser.push(decoder.decode(value, { stream: true }));
        } catch {
          // A frame over the cap: what follows cannot be trusted to start on a frame boundary.
          return { kind: "failed", floorMs: 0, opened };
        }
        if (items.length > 0 && !opened) {
          opened = true;
          this.becomeLive();
        }
        if (opened) this.set({ dataAsOf: this.lastByteAt });
        for (const item of items) this.handle(item);
      }
    } catch {
      if (gen !== this.generation) return { kind: "superseded" };
      return { kind: "failed", floorMs: 0, opened };
    } finally {
      void reader.cancel().catch(() => undefined);
    }
  }

  /** The first frame of a connection arrived: it is live. Decide whether the app has to refetch. */
  private becomeLive(): void {
    const wasOffline = this.snap.status === "offline";
    const hasResumePoint = this.snap.lastEventId !== null;
    this.set({ status: "live", failures: 0, retryAt: null, dataAsOf: this.lastByteAt, stopReason: null });
    if (!hasResumePoint) this.options.onResync("no_id");
    else if (wasOffline) this.options.onResync("was_offline");
  }

  private handle(item: SseItem): void {
    if (item.kind === "retry") {
      this.serverRetryMs = Math.min(item.ms, MAX_FLOOR_MS);
      return;
    }
    if (item.kind === "comment") return;
    const { frame } = item;
    const event = parseDomainEvent(frame.data);
    // A frame is trusted only when its `event:` and its JSON say the same thing. The hub always writes them so.
    if (event !== null && event.type === frame.event) {
      if (event.type === "resync") this.options.onResync("server");
      else {
        try {
          this.options.onEvent(event);
        } catch {
          // A screen that fails on an event must not take the stream with it.
        }
      }
    }
    // Every frame the hub sent is one it will not replay, so the resume point moves even when this client could not use it.
    if (frame.id !== null && validId(frame.id)) this.set({ lastEventId: frame.id });
  }

  // ---- after a connection ends ----

  private settle(end: End, startedAt: number): void {
    if (end.kind === "superseded") return;
    if (end.kind === "halt") {
      this.running = false;
      this.onlineTarget?.removeEventListener("online", this.onOnline);
      this.set({ status: "stopped", stopReason: end.reason, retryAt: null });
      return;
    }
    // A stream that ended before it said anything is a failure, whatever the way it ended.
    const failed = end.kind === "failed" || !end.opened;
    if (this.now() - startedAt >= STABLE_MS) this.backoffAttempt = 0;
    const attempt = this.backoffAttempt;
    this.backoffAttempt += 1;
    const failures = failed ? this.snap.failures + 1 : 0;
    const floorMs = end.kind === "clean" && end.opened ? (this.serverRetryMs ?? DEFAULT_RETRY_HINT_MS) : end.kind === "failed" ? end.floorMs : 0;
    const delay = backoffDelayMs(attempt, { floorMs, random: this.random });
    this.set({
      status: failures >= OFFLINE_AFTER_FAILURES ? "offline" : "reconnecting",
      failures,
      retryAt: this.now() + delay,
    });
    this.clearRetry();
    this.retryTimer = this.timers.setTimeout(() => {
      this.retryTimer = null;
      void this.attempt();
    }, delay);
  }
}
