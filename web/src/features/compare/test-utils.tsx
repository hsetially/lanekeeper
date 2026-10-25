// Helpers for the tests of the compare screen: a hub that is a function of the test, and the rows it serves. Nothing here
// touches the network.
import { vi } from "vitest";
import { apiCaches } from "@/api/client";
import type { GridRow, SettingValue } from "@/api/generated/model";
import { TOKEN } from "@/test/app";

export type Call = { url: URL; method: string; headers: Headers };
export type Handler = (call: Call) => Response | Promise<Response>;

export const json = (body: unknown, status = 200, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", ...headers } });

export const problem = (status: number, type: string, extra: Record<string, unknown> = {}): Response =>
  new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title: type, status, request_id: "req-compare-1", ...extra }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

/** Answer `fetch` with `handler` and keep every call. */
export function stubHub(handler: Handler): Call[] {
  const calls: Call[] = [];
  Object.defineProperty(document, "cookie", { configurable: true, get: () => `__Host-lk_csrf=${TOKEN}`, set: () => undefined });
  apiCaches.clear();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const call: Call = { url: new URL(input, "http://hub.test"), method: (init.method ?? "GET").toUpperCase(), headers: new Headers(init.headers) };
      calls.push(call);
      return handler(call);
    }),
  );
  return calls;
}

export const sv = (text: string, ty: SettingValue["ty"] = "string"): SettingValue => ({ text, ty, flag: null });

/** A row of `lanes` cells that all read `text`, apart from the cells in `over` (a lane index to a text, or null for absent). */
export function row(path: string, lanes: number, text = "5000", over: Record<number, string | null> = {}): GridRow {
  return {
    path,
    cells: Array.from({ length: lanes }, (_, i) => {
      const t = i in over ? over[i] : text;
      return t === null || t === undefined ? { value: null, location: null } : { value: sv(t), location: { path, lines: { start: 10 + i, end: 10 + i } } };
    }),
  };
}

/** A grid of `total` rows served the way the hub serves it: a keyset cursor (here the offset), and at most `limit` a page. */
export function gridServer(total: number, lanes: string[], make: (index: number) => GridRow = (i) => row(`app.setting-${i}`, lanes.length)) {
  const requests: Array<{ limit: number | null; cursor: string | null }> = [];
  const handler: Handler = (call) => {
    if (call.url.pathname !== "/api/v1/grid") return problem(404, "not_found");
    const limit = call.url.searchParams.has("limit") ? Number(call.url.searchParams.get("limit")) : null;
    const cursor = call.url.searchParams.get("cursor");
    requests.push({ limit, cursor });
    const from = cursor === null ? 0 : Number(cursor.replace(/^o/, ""));
    const to = Math.min(total, from + (limit ?? 100));
    return json({ swimlanes: lanes, rows: { items: Array.from({ length: to - from }, (_, i) => make(from + i)), next: to < total ? `o${to}` : null } });
  };
  return { handler, requests };
}
