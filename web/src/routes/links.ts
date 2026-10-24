// URLs from the route table. A screen builds a link with `hrefFor("files", { id: "sit1" }, { path: "a.yml" })` and never
// concatenates a path by hand, so every parameter is checked and encoded the same way and a renamed route fails to compile.

import type { ParamsOf, RouteId, SearchOf } from "./props";
import { stringifySearch } from "./search";
import { paramCheck, paramNames, routeById } from "./table";

export class LinkError extends Error {
  override readonly name = "LinkError";
}

export function hrefFor(routeId: string, params: Readonly<Record<string, string>> = {}, search: Readonly<Record<string, string | undefined>> = {}): string {
  const route = routeById(routeId);
  const names = paramNames(route.path);
  let path = route.path;
  for (const name of names) {
    const value = params[name];
    if (value === undefined) throw new LinkError(`${routeId} needs ${name}`);
    if (paramCheck(route, name)?.(value) === false) throw new LinkError(`${name} of ${routeId} is not valid`);
    path = path.replace(`:${name}`, encodeURIComponent(value));
  }
  const allowed = new Set(route.search ?? []);
  const kept: Record<string, string> = {};
  for (const [key, value] of Object.entries(search)) {
    if (value === undefined) continue;
    if (!allowed.has(key)) throw new LinkError(`${routeId} does not read ${key}`);
    kept[key] = value;
  }
  return `${path}${stringifySearch(kept)}`;
}

/**
 * `hrefFor` with the route's own parameter and search names checked at compile time: `linkTo("files", { id }, { path })`.
 * Screens use this one; `hrefFor` stays loose for the code that works from route ids it reads at run time (the palette, tests).
 */
export function linkTo<Id extends RouteId>(
  routeId: Id,
  ...rest: keyof ParamsOf<Id> extends never ? [params?: undefined, search?: SearchOf<Id>] : [params: ParamsOf<Id>, search?: SearchOf<Id>]
): string {
  const [params, search] = rest;
  return hrefFor(routeId, (params ?? {}), (search ?? {}));
}
