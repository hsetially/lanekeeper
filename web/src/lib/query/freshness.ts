// How long each kind of answer counts as fresh, in one place, with the reason. A feature's `queries.ts` picks one of these
// instead of a number, so the reasoning is written once and a reviewer can see which rule a query follows.

/**
 * Live events keep this fresh: the stream invalidates it the moment something it shows changes (src/live/invalidation.ts), and
 * a resync refetches it after the stream was down. So it is never stale by age; it is stale only when invalidated (and then it
 * refetches at once if it is on screen) or while the stream is down (the stale banner says so).
 */
export const EVENT_FED = Infinity;

/**
 * Nothing tells the app that this changed (a role set by an admin, an audit trail, an admin list): it counts as fresh for a
 * minute, then a screen that shows it says "may be out of date" and offers Refresh, and a screen that mounts it refetches.
 */
export const POLLED_MS = 60 * 1000;

/** Content addressed by its hash is the same forever: never stale, and worth keeping longer than a list. */
export const IMMUTABLE = Infinity;
export const IMMUTABLE_GC_MS = 10 * 60 * 1000;
