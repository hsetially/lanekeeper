// The props of a page, from the route table. Every page of a route is a default export that takes `{ params, search }`:
// `params` are the path parameters (already checked by the router before the page renders, src/routes/table.ts) and `search`
// is what the route's `validateSearch` kept (strings only, only the keys the route reads, src/routes/search.ts). A page
// declares its props as `PageProps<"files">` and gets exact key names for both, so a typo in a search key is a compile error
// and a page never reads the URL by itself.

import type { ROUTE_TABLE } from "./table";

type Def = (typeof ROUTE_TABLE)[number];

/** The id of a route in the table: `"files"`, `"swimlanes"`, `"admin-section"`. */
export type RouteId = Def["id"];

type DefOf<Id extends RouteId> = Extract<Def, { id: Id }>;

type ParamNames<Path extends string> = Path extends `${string}:${infer Name}/${infer Rest}` ? Name | ParamNames<`/${Rest}`> : Path extends `${string}:${infer Name}` ? Name : never;

/** The path parameters of a route, all strings: `ParamsOf<"files">` is `{ readonly id: string }`. */
export type ParamsOf<Id extends RouteId> = { readonly [K in ParamNames<DefOf<Id>["path"]>]: string };

/** The search keys a route reads, all optional strings: `SearchOf<"files">` has `path`, `line`, `diff`, ... */
export type SearchOf<Id extends RouteId> = DefOf<Id> extends { readonly search: readonly (infer K extends string)[] } ? { readonly [P in K]?: string } : { readonly [P in never]?: string };

/** What the router gives a page. */
export type PageProps<Id extends RouteId> = { readonly params: ParamsOf<Id>; readonly search: SearchOf<Id> };
