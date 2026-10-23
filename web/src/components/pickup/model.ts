import { PickupState } from "@/api/generated/model";
import { STATUS_VARIANTS, type StatusVariant } from "@/components/app/StatusBadge";
import { pickupVariant } from "@/features/swimlanes/pickup";
import { formatClock, formatTime } from "@/lib/time";

// When a change takes effect for a consuming service (D85), as words. There are four states and no bare "pending restart": a
// resource file is live within the client cache (and live by a time), a property source needs a notify or a restart, and a new
// folder or a change to channels.yml needs the config-server to restart. This file turns the contract's `PickupState` and the
// times around it into what a chip draws and what its tooltip says. It has no markup in it, so that every sentence can be read in
// a test.
//
// What it will not do:
//  - invent a time: a `live_by` that is absent, not a number, or out of range is "no time", and one that has passed is said to
//    have passed, never presented as still to come;
//  - make a state up: a value that is not one of the four is "Pickup unknown", and the value itself is never repeated (it is the
//    hub's text, and a hub that is wrong, or newer than this app, must not be able to put words on the screen);
//  - say what the notifications flag does: Q35 (do services subscribe?) is open, so the flag is shown as the hub reported it;
//  - map a state to a status colour a second time: `pickupVariant` (features/swimlanes/pickup) is the one mapping.

/** The four states, most urgent first. A summary lists them in this order, and the worst of several is the first that is present. */
export const PICKUP_ORDER: readonly PickupState[] = [PickupState.needs_config_server_restart, PickupState.needs_notify_or_restart, PickupState.live_within_ttl, PickupState.live];

export function isPickupState(value: unknown): value is PickupState {
  return typeof value === "string" && (PICKUP_ORDER as readonly string[]).includes(value);
}

/** 0 for the most urgent state; `PICKUP_ORDER.length` for anything else. */
export function pickupRank(value: unknown): number {
  return isPickupState(value) ? PICKUP_ORDER.indexOf(value) : PICKUP_ORDER.length;
}

/** The two states in which a person has to notify, or restart something. */
export function needsAction(value: unknown): boolean {
  return value === PickupState.needs_notify_or_restart || value === PickupState.needs_config_server_restart;
}

/** The most urgent of the states that were stated. A value that is not one of the four (null, absent, unknown) is not a statement. */
export function worstPickup(states: Iterable<unknown>): PickupState | null {
  let worst: PickupState | null = null;
  for (const state of states) {
    if (isPickupState(state) && (worst === null || pickupRank(state) < pickupRank(worst))) worst = state;
  }
  return worst;
}

// ---- Times ----

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

export type LiveByView = {
  /** `none`: no time to show. `past`: the time has gone. `future`: it has not, or there is no clock to say. */
  readonly kind: "future" | "past" | "none";
  /** `14:32 UTC`, or `2026-10-18 00:05 UTC` when that is not today (UTC). Null for `none`. */
  readonly text: string | null;
  /** With the date and the seconds, for a tooltip. Null for `none`. */
  readonly exact: string | null;
};

const NO_TIME: LiveByView = { kind: "none", text: null, exact: null };

/**
 * Reads `ServiceState.live_by` (milliseconds since the epoch). "By" is a promise about the latest moment, so the time of day is
 * rounded UP to the next minute: 14:32:40 is "by 14:33", never "by 14:32". Everything is UTC, spelled out (lib/time), because
 * the logs are UTC and a screenshot must mean the same to everyone. `now` is the clock the page already has; without it a time is
 * shown as given and is not judged.
 */
export function liveByView(value: unknown, now: number | undefined): LiveByView {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return NO_TIME;
  const rounded = Math.ceil(value / MINUTE) * MINUTE;
  const clock = formatClock(rounded);
  // `formatClock` says "unknown time" for a date the engine cannot show; a real time ends in the zone.
  if (!clock.endsWith(" UTC")) return NO_TIME;
  const clockNow = now !== undefined && Number.isFinite(now) ? now : null;
  const otherDay = clockNow !== null && Math.floor(rounded / DAY) !== Math.floor(clockNow / DAY);
  return {
    kind: clockNow !== null && value <= clockNow ? "past" : "future",
    text: otherDay ? formatTime(rounded) : clock,
    exact: formatTime(value, { seconds: true }),
  };
}

/** A time that can be shown: a number in the range `lib/time` handles. */
export function isShownTime(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && formatClock(value).endsWith(" UTC");
}

/** `cache_ttl_secs` as words: `5 min`, `1 h 30 min`. Null when it is not a whole number of seconds. */
export function formatTtl(secs: unknown): string | null {
  if (typeof secs !== "number" || !Number.isInteger(secs) || secs < 0) return null;
  if (secs < 60) return `${secs} s`;
  if (secs < 3600) return secs % 60 === 0 ? `${secs / 60} min` : `${Math.floor(secs / 60)} min ${secs % 60} s`;
  if (secs % 3600 === 0) return `${secs / 3600} h`;
  const rest = secs % 3600;
  return rest % 60 === 0 ? `${Math.floor(secs / 3600)} h ${rest / 60} min` : `${Math.floor(secs / 3600)} h ${Math.floor(rest / 60)} min ${rest % 60} s`;
}

// ---- The notifications flag (Q35) ----

/**
 * What the hub reported about notifications, and no more. Whether services subscribe is an open question (Q35), so nothing here
 * says what a notification does for a service: the flag is shown as it was reported.
 */
export function notificationsText(enabled: unknown): string {
  if (enabled === true) return "Notifications: on (as reported by the hub)";
  if (enabled === false) return "Notifications: off (as reported by the hub)";
  return "Notifications: not reported by the hub";
}

// ---- One chip ----

/** What a chip stands for: one service, one file, or a number of services in the same state. */
export type PickupSubject = "service" | "file" | "count";

export type PickupViewOptions = {
  readonly subject: PickupSubject;
  /** `ServiceState.live_by`. `undefined`: the data has none to give (a draft's list of services), so nothing is said about it. */
  readonly liveBy?: unknown;
  /** The page's clock, to tell a time to come from a time that has passed. */
  readonly now?: number | undefined;
  /** For `count`: how many services. */
  readonly count?: number | undefined;
};

export type PickupView = {
  readonly state: PickupState | "unrecognised";
  /** The status badge variant (`pickupVariant`); `unknown` for a state that is not one of the four. */
  readonly variant: StatusVariant;
  /** What the badge draws, whole: "Live by 14:32 UTC". */
  readonly text: string;
  /** For the `live-by` variant: the time the badge appends to "Live by". Undefined where `label` says everything. */
  readonly detail: string | undefined;
  /** The badge's own text when it is not the variant's: "Live within client cache", "Pickup unknown". */
  readonly label: string | undefined;
  /** What goes beside the badge in smaller type: "(client cache)", "(time passed)". */
  readonly qualifier: string | null;
  /** The accessible name: the text and the qualifier. */
  readonly name: string;
  /** When the change takes effect, in plain words (the tooltip). */
  readonly words: string;
  /** The service needs a person to notify or restart something. */
  readonly action: boolean;
};

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;

function wordsFor(state: PickupState, options: PickupViewOptions, by: LiveByView): string {
  const { subject } = options;
  const count = options.count ?? 0;
  switch (state) {
    case PickupState.live:
      return subject === "file" ? "Nothing is waiting on this file." : "This service already has the current config.";
    case PickupState.live_within_ttl: {
      if (subject === "file") return "A resource file: services read the new version when their client cache expires.";
      if (subject === "count") return `${plural(count, "service reads", "services read")} a changed resource file again when ${count === 1 ? "its" : "their"} client cache expires.`;
      if (by.kind === "past") return `A resource file changed. Its client cache should have expired by ${by.exact ?? "the time the hub gave"}, and the hub has not reported this service as live yet.`;
      if (by.kind === "future") return `A resource file changed. This service reads it again when its client cache expires, by ${by.exact ?? "the time the hub gave"}.`;
      return options.liveBy === undefined
        ? "A resource file changed. This service reads it again when its client cache expires."
        : "A resource file changed. This service reads it again when its client cache expires. The hub did not say when.";
    }
    case PickupState.needs_notify_or_restart:
      if (subject === "file") return "A property file: services take the new version after they are notified or restarted.";
      if (subject === "count") return `${plural(count, "service needs", "services need")} a notify or a restart to take a changed property file.`;
      return "A property file changed. This service takes it after it is notified or restarted.";
    case PickupState.needs_config_server_restart:
      if (subject === "file") return "In a new folder or channels.yml: the config-server does not serve it until it restarts.";
      if (subject === "count") return `${plural(count, "service waits", "services wait")} for the config-server to restart (a new folder or a change to channels.yml).`;
      return "A new folder or a change to channels.yml. The config-server does not serve it until the config-server restarts.";
  }
}

const UNRECOGNISED_WORDS = "The hub reported a pickup state that this version of the app does not know, so it cannot say when the change takes effect.";
const WITHIN_CACHE = "Live within client cache";

/** What a chip for `state` draws and says. A value that is not one of the four states is "Pickup unknown" and is never repeated. */
export function pickupView(state: unknown, options: PickupViewOptions): PickupView {
  if (!isPickupState(state)) {
    return { state: "unrecognised", variant: "unknown", text: "Pickup unknown", detail: undefined, label: "Pickup unknown", qualifier: null, name: "Pickup unknown", words: UNRECOGNISED_WORDS, action: false };
  }
  const variant = pickupVariant(state);
  const action = needsAction(state);
  if (state !== PickupState.live_within_ttl || options.subject === "count") {
    // The words of a variant are the badge's own (STATUS_VARIANTS); only the live-by badge needs a time, which a count has none of.
    const withinCache = state === PickupState.live_within_ttl;
    const text = withinCache ? WITHIN_CACHE : STATUS_VARIANTS[variant].label;
    return { state, variant, text, detail: undefined, label: withinCache ? WITHIN_CACHE : undefined, qualifier: null, name: text, words: wordsFor(state, options, NO_TIME), action };
  }
  const by = liveByView(options.liveBy, options.now);
  const words = wordsFor(state, options, by);
  if (by.kind === "none" || by.text === null) {
    return { state, variant, text: WITHIN_CACHE, detail: undefined, label: WITHIN_CACHE, qualifier: null, name: WITHIN_CACHE, words, action };
  }
  const qualifier = by.kind === "past" ? "(time passed)" : "(client cache)";
  const text = `Live by ${by.text}`;
  return { state, variant, text, detail: by.text, label: undefined, qualifier, name: `${text} ${qualifier}`, words, action };
}

// ---- Counts ----

export type PickupCountPairs = readonly (readonly [PickupState, number])[] | undefined;

const whole = (n: unknown): number => (typeof n === "number" && Number.isInteger(n) && n > 0 ? n : 0);

/**
 * The pairs of `SwimlaneSummary.pickup_counts` to draw: each state once (a repeat is added), most urgent first, with only whole
 * positive counts. The contract lists the states that are not live, so `live` and anything unknown are dropped. `actionable` keeps
 * the two states that need a person; `all` adds the ones that pick the change up by themselves.
 */
export function countsToShow(counts: PickupCountPairs, include: "actionable" | "all"): Array<[PickupState, number]> {
  const sums = new Map<PickupState, number>();
  for (const pair of counts ?? []) {
    const [state, n] = pair;
    if (!isPickupState(state) || state === PickupState.live) continue;
    if (include === "actionable" && !needsAction(state)) continue;
    const add = whole(n);
    if (add > 0) sums.set(state, (sums.get(state) ?? 0) + add);
  }
  return PICKUP_ORDER.flatMap((state): Array<[PickupState, number]> => {
    const n = sums.get(state);
    return n === undefined ? [] : [[state, n]];
  });
}
