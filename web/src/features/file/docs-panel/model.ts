import type { FlagStatus } from "@/api/generated/model";
import { linkTo } from "@/routes/links";

// Pure helpers of the docs panel: the address of a doc at a heading, and one row of the Feature Flags table.

const MAX_DOC_PATH = 1024;
const MAX_ANCHOR = 80;
// Control characters, bidi and invisible format characters, line and paragraph separators and backslashes have no place in a
// doc path (the same rule the doc reader applies to a path from the address bar).
const UNSAFE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\\]/u;
/** The shape of a heading id the server gives (`DocHit.anchor`). A client never builds one from a title. */
const HEADING_ID = /^doc-[a-z0-9]+(-[a-z0-9]+)*$/;

function isDocPath(path: string): boolean {
  if (path.length === 0 || path.length > MAX_DOC_PATH || UNSAFE.test(path)) return false;
  const source = path.startsWith("/git/") ? "/git/" : path.startsWith("/uploads/") ? "/uploads/" : undefined;
  if (source === undefined) return false;
  return path.slice(source.length).split("/").every((segment) => segment !== "" && segment !== "." && segment !== "..");
}

/**
 * Where the reader opens `path`, at the heading `anchor` when the server gave one of the right shape. The path is handed over
 * as it came: nothing here decodes a percent escape (a `%2e%2e` in a path stays text, and is encoded once more in the address).
 * A path that is not a doc path gives no link, so the title is shown and nothing points anywhere else.
 */
export function docLink(path: string, anchor: string | null | undefined): string | null {
  if (!isDocPath(path)) return null;
  const base = linkTo("doc-view", undefined, { path });
  return anchor !== null && anchor !== undefined && anchor.length <= MAX_ANCHOR && HEADING_ID.test(anchor) ? `${base}#${anchor}` : base;
}

export type FlagRow = {
  /** The setting path the flag sets. */
  setting: string;
  /** The flag's name. */
  flag: string;
  /** The value in this swimlane, or null when the file does not set it there. */
  value: string | null;
  /** The template's value, or null when it has none. */
  fallback: string | null;
  /** The value is not the default: shown in bold. */
  differs: boolean;
};

/** One row for a flag, with the value this swimlane has. */
export function flagRow(flag: FlagStatus, lane: string): FlagRow {
  const value = flag.values.find((v) => v.swimlane === lane)?.value?.text ?? null;
  const fallback = flag.template_default?.text ?? null;
  return { setting: flag.setting, flag: flag.flag, value, fallback, differs: value !== fallback };
}

/** How many flags the panel draws. The contract allows 1,000; a file has a handful. */
export const MAX_FLAG_ROWS = 200;
