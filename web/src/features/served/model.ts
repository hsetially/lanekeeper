import { isChannelName } from "@/features/compare/model";
import { isPlausibleNfsPath } from "@/features/file/model";
import { linkTo } from "@/routes/links";

// The plain work of the "As served" view: which values of the address (the swimlane's `/served` route) can be a request at all, what is
// said next to a field that cannot, and the link to a selection. The address is untrusted input (anyone can send a link) and the hub checks
// every value again; these checks only keep a value that cannot be valid off the wire. Nothing here decides a fact about a file: the
// served text, its role and its sources are what the hub and the engine say.

export type ServedField = "file" | "application" | "tenant" | "channel";
export const SERVED_FIELDS: readonly ServedField[] = ["file", "application", "tenant", "channel"];

/** What the four fields hold (text, as typed or as the address had it). */
export type ServedDraft = Readonly<Record<ServedField, string>>;
export type ServedProblems = Readonly<Partial<Record<ServedField, string>>>;

/** One exact request for `getServedFile`: the swimlane is the page's. No channel is `null`. */
export type ServedSelection = {
  readonly file: string;
  readonly application: string;
  readonly tenant: string;
  readonly channel: string | null;
};

/** The search keys of the route, all strings that may be missing. */
export type ServedSearch = { readonly [K in ServedField]?: string | undefined };

// AppName and TenantId in api/openapi.yaml. A channel has the same pattern as an application and the compare screen already has its
// check (`isChannelName`), so that one is reused; a file is an NfsPath, which the file view already checks (`isPlausibleNfsPath`).
const APP_NAME = /^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$/;
const TENANT_ID = /^[a-z0-9][a-z0-9_-]{0,62}$/;

export const isAppName = (text: string): boolean => APP_NAME.test(text);
export const isTenantId = (text: string): boolean => TENANT_ID.test(text);

const NAME_HELP = "Use letters, digits, . _ and -, starting with a letter, digit or _, at most 128 characters";

const REQUIRED: Readonly<Record<Exclude<ServedField, "channel">, string>> = {
  file: "A file is required",
  application: "An application is required",
  tenant: "A tenant is required",
};

const clean = (text: string | undefined): string => (text ?? "").trim();

/**
 * What is wrong with one field's value, or null. The file, the application and the tenant are required; the channel may be empty (a
 * request without a channel). The message never repeats the value it was given.
 */
export function fieldProblem(field: ServedField, value: string): string | null {
  const text = clean(value);
  if (text === "") return field === "channel" ? null : REQUIRED[field];
  switch (field) {
    case "file":
      return isPlausibleNfsPath(text) ? null : "That is not a path below the swimlane's root. Use a relative path such as tx-infinity-api/tx-infinity-core.yml, with no .. and no leading slash";
    case "application":
      return isAppName(text) ? null : NAME_HELP;
    case "tenant":
      return isTenantId(text) ? null : "Use lower case letters, digits, - and _, starting with a letter or digit, at most 63 characters";
    case "channel":
      return isChannelName(text) ? null : NAME_HELP;
  }
}

/** Every field that is wrong, with why: the required ones that are empty included. */
export function problemsOf(draft: ServedDraft): ServedProblems {
  const out: Partial<Record<ServedField, string>> = {};
  for (const field of SERVED_FIELDS) {
    const problem = fieldProblem(field, draft[field]);
    if (problem !== null) out[field] = problem;
  }
  return out;
}

/** The request a draft stands for, or null while any field is wrong or missing. Values are trimmed; an empty channel is no channel. */
export function selectionFrom(draft: ServedDraft): ServedSelection | null {
  if (Object.keys(problemsOf(draft)).length > 0) return null;
  const channel = clean(draft.channel);
  return { file: clean(draft.file), application: clean(draft.application), tenant: clean(draft.tenant), channel: channel === "" ? null : channel };
}

export type ReadSelection = {
  /** What each field holds, as the address had it (empty for a key that was not given). */
  readonly draft: ServedDraft;
  /** The request, or null while the address is not a complete, valid one. */
  readonly selection: ServedSelection | null;
  /** Only for a key the address GAVE with a value that cannot be valid. A key that was never given is not scolded for. */
  readonly problems: ServedProblems;
};

export function readSelection(search: ServedSearch): ReadSelection {
  const draft: ServedDraft = { file: clean(search.file), application: clean(search.application), tenant: clean(search.tenant), channel: clean(search.channel) };
  const problems: Partial<Record<ServedField, string>> = {};
  for (const field of SERVED_FIELDS) {
    if (draft[field] === "") continue;
    const problem = fieldProblem(field, draft[field]);
    if (problem !== null) problems[field] = problem;
  }
  return { draft, selection: selectionFrom(draft), problems };
}

export function sameSelection(a: ServedSelection | null, b: ServedSelection | null): boolean {
  if (a === null || b === null) return a === b;
  return a.file === b.file && a.application === b.application && a.tenant === b.tenant && a.channel === b.channel;
}

/** The address of the view for a selection. The channel is left out when there is none. */
export function servedHref(swimlane: string, selection: ServedSelection): string {
  return linkTo(
    "swimlane-served",
    { id: swimlane },
    { application: selection.application, tenant: selection.tenant, file: selection.file, ...(selection.channel === null ? {} : { channel: selection.channel }) },
  );
}

/**
 * The placeholders to name: each once, the first `max`, and how many more there are. A text can leave up to 10,000 of them, and the list
 * is only to say which they are; the text itself shows every one.
 */
export function placeholdersShown(placeholders: readonly string[], max: number): { readonly shown: string[]; readonly more: number } {
  const unique = [...new Set(placeholders)];
  const shown = unique.slice(0, max);
  return { shown, more: unique.length - shown.length };
}
