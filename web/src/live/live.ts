import type { QueryClient } from "@tanstack/react-query";
import { InvalidationBatcher } from "./apply";
import { plan, type Notice } from "./invalidation";
import { LiveStream, type LiveStreamOptions } from "./sse-client";

// The live layer without React: the stream, the plans it feeds, and the batcher that applies them to a QueryClient.
// `LiveProvider` owns one of these for the whole tab. It has no state of its own beyond what its parts keep.

export type LiveConfig = {
  /** False: never open a stream, whatever the session says. For the tests of everything that is not the live layer. */
  enabled?: boolean;
  /** The transport's own settings (the fetch, the clock, the random draw, the timers). Tests supply theirs. */
  stream?: Partial<Pick<LiveStreamOptions, "fetch" | "now" | "random" | "timers" | "stallMs" | "url" | "onlineTarget" | "onProblem">>;
  /** The coalescing window of the invalidation, in milliseconds. */
  windowMs?: number;
};

const MAX_NOTICE_LISTENERS = 8;

export class Live {
  readonly stream: LiveStream;
  private readonly batcher: InvalidationBatcher;
  private readonly noticeListeners = new Set<(notice: Notice) => void>();

  constructor(client: QueryClient, config: LiveConfig = {}) {
    this.batcher = new InvalidationBatcher(client, {
      onNotice: (notice) => {
        for (const listener of [...this.noticeListeners]) {
          try {
            listener(notice);
          } catch {
            // One listener failing must not hide the notice from the others.
          }
        }
      },
      ...(config.windowMs === undefined ? {} : { windowMs: config.windowMs }),
    });
    this.stream = new LiveStream({
      ...config.stream,
      onEvent: (event) => {
        this.batcher.push(plan(event));
      },
      onResync: () => {
        this.batcher.pushAll();
      },
    });
  }

  /** Listen for notices (a change that did not come through this app, a proposal that changed). At most 8 listeners (rule 5). */
  onNotice(listener: (notice: Notice) => void): () => void {
    if (this.noticeListeners.size >= MAX_NOTICE_LISTENERS) throw new Error("too many live notice listeners");
    this.noticeListeners.add(listener);
    return () => {
      this.noticeListeners.delete(listener);
    };
  }

  start(): void {
    this.stream.start();
  }

  /** Close the stream and drop what is waiting to be applied. It can be started again. */
  stop(): void {
    this.stream.stop();
    this.batcher.dispose();
  }
}
