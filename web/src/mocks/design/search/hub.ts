// A fake hub for the tests of the Search, Docs and Feature Status screens: the session and the sidebar's data, and the routes a
// test adds. Test code only (it imports the app test helper); the scenarios of ./index.ts never load it.
import { json, me, problem, type Api, type Call } from "@/test/app";
import { DOCS, FLAGS, docHitsFor, settingHitsFor, textHitsFor } from "./data.ts";
import { SHELL_LANES } from "../shell/index.ts";

export type Route = (call: Call, url: URL) => Response | undefined;

/** A route for one method and path (a string is the whole path, a regular expression is tested against it). */
export function at(method: "GET" | "POST" | "PUT" | "DELETE", path: string | RegExp, respond: (call: Call, url: URL) => Response): Route {
  return (call, url) => {
    if (call.method !== method) return undefined;
    const hit = typeof path === "string" ? url.pathname === path : path.test(url.pathname);
    return hit ? respond(call, url) : undefined;
  };
}

/** The app around a screen: who is signed in, the six swimlanes of the design, and an empty approval queue. */
export function hub(role: string | null, routes: readonly Route[] = []): Api {
  return (call) => {
    const url = new URL(call.path, "http://hub.test");
    if (url.pathname === "/api/v1/me") return json(me(role));
    if (url.pathname === "/api/v1/swimlanes") return json({ items: SHELL_LANES, next: null });
    if (url.pathname === "/api/v1/proposals") return json({ items: [], next: null });
    if (url.pathname === "/api/v1/me/github-token") return problem(404, "not_found");
    if (url.pathname.startsWith("/api/v1/me/mcp-tokens")) return problem(404, "not_found");
    for (const route of routes) {
      const response = route(call, url);
      if (response) return response;
    }
    return problem(404, "not_found");
  };
}

/** The paths the screens asked for, in order: `GET /api/v1/docs/fs/render?path=...`. */
export function asked(calls: readonly Call[], prefix = "/api/v1/"): string[] {
  return calls.filter((c) => c.path.startsWith(prefix) && !c.path.startsWith("/api/v1/me") && c.path !== "/api/v1/swimlanes" && !c.path.startsWith("/api/v1/proposals")).map((c) => `${c.method} ${c.path}`);
}

const lanesOf = (url: URL): string[] | undefined => {
  const raw = url.searchParams.get("swimlanes");
  return raw === null ? undefined : raw.split(",").filter(Boolean);
};

/**
 * The searches of the page over the fixture data, answered as the server answers them: text search with its cap and
 * `truncated`, settings search by keyset pages, docs search by `limit`, and the list of docs. A bad regex is a 400.
 */
export function searchBackend(options: { pageSize?: number; textCap?: number } = {}): Route[] {
  const textCap = options.textCap ?? 500;
  return [
    at("GET", "/api/v1/search/text", (_call, url) => {
      const lanes = lanesOf(url);
      const glob = url.searchParams.get("path_glob") ?? undefined;
      try {
        const hits = textHitsFor({
          pattern: url.searchParams.get("pattern") ?? "",
          regex: url.searchParams.get("regex") === "true",
          caseSensitive: url.searchParams.get("case_sensitive") === "true",
          ...(lanes === undefined ? {} : { lanes }),
          ...(glob === undefined ? {} : { glob }),
        });
        const cap = Math.min(Number(url.searchParams.get("max_total") ?? textCap), textCap);
        return json({ hits: hits.slice(0, cap), truncated: hits.length > cap });
      } catch {
        return problem(400, "bad_request");
      }
    }),
    at("GET", "/api/v1/search/settings", (_call, url) => {
      const lanes = lanesOf(url);
      const all = settingHitsFor(url.searchParams.get("q") ?? "", lanes);
      const size = Math.min(Number(url.searchParams.get("limit") ?? options.pageSize ?? 100), options.pageSize ?? 100);
      const from = Number((url.searchParams.get("cursor") ?? "c0").slice(1));
      const items = all.slice(from, from + size);
      return json({ items, next: from + size < all.length ? `c${from + size}` : null });
    }),
    at("GET", "/api/v1/docs/search", (_call, url) => json({ hits: docHitsFor(url.searchParams.get("q") ?? "").slice(0, Number(url.searchParams.get("limit") ?? 10)) })),
    at("GET", "/api/v1/docs", (_call, url) => {
      const source = url.searchParams.get("source");
      const prefix = url.searchParams.get("path_prefix") ?? "";
      return json({ items: DOCS.filter((d) => (source === null || d.source === source) && d.path.startsWith(prefix)), next: null });
    }),
  ];
}

/** The feature status of the page: every documented flag, with its value in every swimlane. */
export function featureBackend(flags = FLAGS): Route[] {
  return [at("GET", "/api/v1/features/status", () => json({ flags }))];
}
