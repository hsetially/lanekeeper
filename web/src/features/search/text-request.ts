import type { SearchTextParams } from "@/api/generated/model";
import { utf8Length } from "@/lib/text/byte-ranges";
import type { SearchParams } from "./url";

// The request a text search makes, from the parameters in the address. Only what differs from the server's defaults is sent: a
// literal, case-insensitive search of every swimlane has the pattern and nothing else.

/** The longest pattern the server takes (`pattern` is "at most 512 bytes"). */
export const MAX_PATTERN_BYTES = 512;
/** The longest path glob (`ShortText`). */
export const MAX_GLOB = 256;

export type TextProblem = { kind: "blank" } | { kind: "no_lanes" } | { kind: "too_long"; bytes: number } | { kind: "glob_too_long" };

/** Why the screen does not ask, or undefined when it asks. */
export function textProblem(params: SearchParams): TextProblem | undefined {
  if (params.q === "") return { kind: "blank" };
  if (params.lanes !== null && params.lanes.length === 0) return { kind: "no_lanes" };
  const bytes = utf8Length(params.q);
  if (bytes > MAX_PATTERN_BYTES) return { kind: "too_long", bytes };
  if (params.glob.length > MAX_GLOB) return { kind: "glob_too_long" };
  return undefined;
}

export function textParams(params: SearchParams, extra: { max_total?: number } = {}): SearchTextParams {
  return {
    pattern: params.q,
    ...(params.regex ? { regex: true } : {}),
    ...(params.caseSensitive ? { case_sensitive: true } : {}),
    ...(params.lanes !== null ? { swimlanes: params.lanes } : {}),
    ...(params.glob !== "" ? { path_glob: params.glob } : {}),
    ...extra,
  };
}

/** The request's identity, for the one-shot hook: the same parameters are the same request. */
export function textKey(params: SearchParams, extra = ""): string {
  return JSON.stringify(["text", params.q, params.regex, params.caseSensitive, params.lanes, params.glob, extra]);
}
