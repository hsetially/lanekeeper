import { PickupState, type NfsPath, type ServiceState } from "@/api/generated/model";

// What a "Notify services" request carries, worked out from what the agent reports about the services and from nothing else: the
// files they are waiting on (`ServiceState.pending_files`). The contract's request holds 1 to 100 distinct paths. The hub
// decides what a notification does; this only says which files the person is about to announce.

/** `NotifyRequest.paths` holds at most this many. */
export const MAX_NOTIFY_PATHS = 100;

export type NotifyPlan = {
  /** The paths to send, each once, in a fixed order. At most `MAX_NOTIFY_PATHS`. */
  readonly paths: readonly NfsPath[];
  /** How many more files are waiting than this request carries. They are not lost: they are simply not in this request. */
  readonly left: number;
  /** The services that wait on at least one of `paths`, by namespace and name. */
  readonly services: readonly ServiceState[];
};

/**
 * A notification helps a service that waits for "a notify or a restart". A service that will go live within its cache time does
 * not need one, and one that waits for the config-server to restart is not helped by a notification (D85).
 */
const waitsForNotify = (service: ServiceState): boolean => service.pickup === PickupState.needs_notify_or_restart;

const byName = (a: ServiceState, b: ServiceState): number => a.service.namespace.localeCompare(b.service.namespace) || a.service.name.localeCompare(b.service.name);

function plan(candidates: readonly ServiceState[], all: readonly NfsPath[]): NotifyPlan {
  const distinct = [...new Set(all)].sort();
  const paths = distinct.slice(0, MAX_NOTIFY_PATHS);
  const sent = new Set(paths);
  const services = candidates.filter((service) => service.pending_files.some((file) => sent.has(file))).sort(byName);
  return { paths, left: distinct.length - paths.length, services };
}

/** Every file that a service is waiting for a notify or a restart on: the request of the Services tab. */
export function planForServices(services: readonly ServiceState[]): NotifyPlan {
  const waiting = services.filter(waitsForNotify);
  return plan(
    waiting,
    waiting.flatMap((service) => service.pending_files),
  );
}

/** One file that was just written: the request of the file page. The services listed are the ones waiting on it, if any are. */
export function planForFile(path: NfsPath, services: readonly ServiceState[]): NotifyPlan {
  return { ...plan(services.filter(waitsForNotify), [path]), paths: [path] };
}
