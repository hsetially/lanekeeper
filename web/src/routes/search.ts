// Search parameters, the way the app reads and writes them. TanStack Router's default parses every value as JSON, so
// `?q=123` would arrive as the number 123 and `?x={"a":1}` as an object (a surprise in a search box, and a way to put
// structured data where a string is expected). Here every value is a string, keys are bounded, and the keys of
// Object.prototype can never be set. Each route then names the keys it reads (`pickSearch`) and drops the rest.

import { validateNext, validateRequestId } from "@/auth/params";
import { ROUTES } from "./table";

export const MAX_SEARCH_PARAMS = 32;
export const MAX_SEARCH_KEY = 64;
export const MAX_SEARCH_VALUE = 512;
const FORBIDDEN_KEYS = new Set(["__proto__", "constructor", "prototype"]);

function decode(text: string): string {
  try {
    return decodeURIComponent(text.replaceAll("+", " "));
  } catch {
    return text;
  }
}

export function parseSearch(search: string, only?: ReadonlySet<string>): Record<string, string> {
  const out: Record<string, string> = {};
  const text = search.startsWith("?") ? search.slice(1) : search;
  if (text === "") return out;
  let count = 0;
  for (const part of text.split("&")) {
    if (part === "") continue;
    const eq = part.indexOf("=");
    const key = decode(eq < 0 ? part : part.slice(0, eq));
    if (key === "" || key.length > MAX_SEARCH_KEY || FORBIDDEN_KEYS.has(key)) continue;
    if (only && !only.has(key)) continue;
    const value = decode(eq < 0 ? "" : part.slice(eq + 1)).slice(0, MAX_SEARCH_VALUE);
    // `next` and `req` mean the same on every route, so they are checked where they are read from the URL: an invalid one
    // is never in the router's state, and a screen cannot forget to validate it.
    if (key === "next" && validateNext(value) === undefined) continue;
    if (key === "req" && validateRequestId(value) === undefined) continue;
    if (!(key in out)) {
      if (count >= MAX_SEARCH_PARAMS) continue;
      count += 1;
    }
    out[key] = value;
  }
  return out;
}

const encode = (text: string): string => encodeURIComponent(text).replaceAll("%20", "+");

export function stringifySearch(search: Record<string, unknown>): string {
  const parts: string[] = [];
  for (const key of Object.keys(search).sort()) {
    const value = search[key];
    if (typeof value !== "string" || value === "") continue;
    parts.push(`${encode(key)}=${encode(value)}`);
  }
  return parts.length === 0 ? "" : `?${parts.join("&")}`;
}

/** A `validateSearch` that keeps the named keys as strings. `next` and `req` get their own validators (src/auth/params.ts). */
export function pickSearch(keys: readonly string[]): (raw: Record<string, unknown>) => Record<string, string> {
  return (raw) => {
    const out: Record<string, string> = {};
    for (const key of keys) {
      const value = raw[key];
      if (typeof value !== "string") continue;
      const checked = key === "next" ? validateNext(value) : key === "req" ? validateRequestId(value) : value;
      if (checked !== undefined) out[key] = checked;
    }
    return out;
  };
}

/** The keys any route reads. A key outside this list never enters the router's state. */
export const KNOWN_SEARCH_KEYS: ReadonlySet<string> = new Set([...ROUTES.flatMap((route) => route.search ?? []), "next", "req"]);

/** What the router uses: strings only, known keys only, `next` and `req` validated. */
export const parseAppSearch = (search: string): Record<string, string> => parseSearch(search, KNOWN_SEARCH_KEYS);
