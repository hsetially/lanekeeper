import type { QueryClient } from "@tanstack/react-query";
import { invalidateTargets, targetId, type Target } from "@/lib/query/targets";
import type { Notice, Plan } from "./invalidation";

// Applying plans to the QueryClient. Events arrive in bursts (a scan observes a hundred files), so plans are coalesced over a
// window: the first event starts it, every plan inside it adds its targets to a set (deduplicated by name), and at the end
// ONE `invalidateQueries` runs with a predicate that tests each cache entry against the whole set. `refetchType: "active"`
// refetches what a screen is showing and only marks the rest stale. A window that names more than MAX_TARGETS distinct things
// is not worth testing entry by entry, and falls back to "every active query" (immutable entries are still never touched).
//
// Notices go straight to their listeners: a toast should not wait for the window.

/** One window. A live update still reaches the screen well inside P13's second. */
export const WINDOW_MS = 100;
export const MAX_TARGETS = 200;

type Handle = ReturnType<typeof setTimeout>;

export type BatcherOptions = {
  onNotice?: (notice: Notice) => void;
  windowMs?: number;
  maxTargets?: number;
};

export class InvalidationBatcher {
  private readonly pending = new Map<string, Target>();
  private everything = false;
  private timer: Handle | null = null;
  private readonly windowMs: number;
  private readonly maxTargets: number;

  constructor(
    private readonly client: QueryClient,
    private readonly options: BatcherOptions = {},
  ) {
    this.windowMs = options.windowMs ?? WINDOW_MS;
    this.maxTargets = options.maxTargets ?? MAX_TARGETS;
  }

  push(plan: Plan): void {
    for (const notice of plan.notices) {
      try {
        this.options.onNotice?.(notice);
      } catch {
        // A toaster that fails must not stop the data from refreshing.
      }
    }
    if (plan.all) this.everything = true;
    // Past the limit nothing more is remembered one by one: the window is going to refetch every active query anyway.
    if (!this.everything && this.pending.size <= this.maxTargets) {
      for (const target of plan.targets) this.pending.set(targetId(target), target);
    }
    this.schedule();
  }

  /** A resync: every active query that is not immutable. */
  pushAll(): void {
    this.everything = true;
    this.schedule();
  }

  private schedule(): void {
    if (this.timer !== null) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.flush();
    }, this.windowMs);
  }

  flush(): void {
    const targets = [...this.pending.values()];
    const everything = this.everything || targets.length > this.maxTargets;
    this.pending.clear();
    this.everything = false;
    if (!everything && targets.length === 0) return;
    void invalidateTargets(this.client, everything ? "everything" : targets);
  }

  /** Drop what is waiting and stop the timer. */
  dispose(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.pending.clear();
    this.everything = false;
  }
}
