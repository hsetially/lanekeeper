import type { DomainEvent } from "@/api/generated/model";

// What a frame of the live stream may say (S11: the stream is untrusted input like any other). The contract's DomainEvent is
// a closed `oneOf` of the kinds below, every field required, so a frame is accepted only when its `type` is one of them and
// each field of that kind has the type the contract gives it. The table is typed by the generated union: a kind added to the
// contract without a row here is a compile error, and events.test.ts compares the rows with the schema's own field lists.
//
// Values are not checked for shape beyond their JSON type (a swimlane id is a string): they are only ever compared with the
// parts of query keys (src/lib/query/keys.ts), never rendered, parsed as a path, or used to build a URL.

type Fields = Record<string, unknown>;
type Check = (fields: Fields) => boolean;

/** The contract caps every list of an event at this many items. */
const MAX_LIST = 1000;

const str = (v: unknown): v is string => typeof v === "string";
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const list = (v: unknown, item?: (x: unknown) => boolean): v is unknown[] => Array.isArray(v) && v.length <= MAX_LIST && (item === undefined || v.every(item));
const obj = (v: unknown): v is Fields => typeof v === "object" && v !== null && !Array.isArray(v);
const jobRef = (v: unknown): boolean => obj(v) && str(v["name"]) && str(v["uid"]);
const serviceRef = (v: unknown): boolean => obj(v) && str(v["namespace"]) && str(v["name"]);

const CHECKS: Record<DomainEvent["type"], Check> = {
  file_observed: (e) => str(e["swimlane"]) && str(e["path"]) && str(e["hash"]) && str(e["source"]),
  drift_changed: (e) => str(e["swimlane"]) && list(e["paths"], str),
  findings_changed: (e) => str(e["swimlane"]),
  pending_restart_changed: (e) => str(e["swimlane"]) && list(e["services"], serviceRef),
  proposal_changed: (e) => num(e["id"]) && str(e["status"]),
  agent_status_changed: (e) => str(e["swimlane"]) && str(e["status"]),
  git_head_moved: (e) => str(e["repo"]) && str(e["branch"]) && (e["from"] === null || str(e["from"])) && str(e["to"]),
  access_request_created: (e) => str(e["user"]),
  doc_indexed: (e) => num(e["doc"]),
  resync: () => true,
  attribution_updated: (e) => str(e["swimlane"]) && str(e["path"]) && str(e["hash"]),
  pr_state_changed: (e) => num(e["link_id"]) && str(e["state"]),
  pr_job_progress: (e) => num(e["job_id"]) && str(e["state"]),
  sync_window_opened: (e) => str(e["swimlane"]) && jobRef(e["job"]),
  sync_window_closed: (e) => str(e["swimlane"]) && jobRef(e["job"]),
  pickup_changed: (e) => str(e["swimlane"]) && list(e["services"], serviceRef),
  swimlane_changed: (e) => str(e["swimlane"]),
};

/** Every kind the client understands, in the contract's order. */
export const EVENT_KINDS = Object.keys(CHECKS) as ReadonlyArray<DomainEvent["type"]>;

function isKind(value: unknown): value is DomainEvent["type"] {
  return typeof value === "string" && Object.hasOwn(CHECKS, value);
}

/** The event a `data:` line holds, or null when it is not JSON, not an object, of an unknown kind, or a field is off. */
export function parseDomainEvent(text: string): DomainEvent | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!obj(value)) return null;
  const type = value["type"];
  if (!isKind(type) || !CHECKS[type](value)) return null;
  return value as unknown as DomainEvent;
}
