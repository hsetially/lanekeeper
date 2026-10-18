import { stringifySearch } from "./search";

// Where a route that only redirects sends the visitor (an alias, or a link the MCP tools and Teams cards send). The target can
// keep the visitor's search keys under the names it reads, and can add keys of its own.

export type Redirect = {
  /** The target's path. `:name` is filled from the route's own path parameters of that name. */
  to: string;
  /** Search keys written on the target as they are. */
  with?: Readonly<Record<string, string>>;
  /** The visitor's search keys carried to the target, `{ read: written }`. Only keys the redirecting route reads are there to carry. */
  carry?: Readonly<Record<string, string>>;
};

/** `/swimlanes/:id` with `{ id: "sit1" }` is `/swimlanes/sit1`. A value is encoded, so it cannot change the shape of the path. */
export function fillParams(path: string, params: Readonly<Record<string, string>>): string {
  return path.replace(/:([A-Za-z]+)/g, (_match, name: string) => encodeURIComponent(params[name] ?? ""));
}

export function redirectHref(redirect: Redirect, params: Readonly<Record<string, string>>, search: Readonly<Record<string, string | undefined>>): string {
  const out: Record<string, string> = {};
  for (const [read, written] of Object.entries(redirect.carry ?? {})) {
    const value = search[read];
    if (value !== undefined && value !== "") out[written] = value;
  }
  Object.assign(out, redirect.with);
  return `${fillParams(redirect.to, params)}${stringifySearch(out)}`;
}
