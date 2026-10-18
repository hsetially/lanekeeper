// A design scenario as the fake server of a unit test: the same answers the e2e harness gives, matched by method and OpenAPI
// path, so a test of the file screen reads the data the design-parity screens are built from. Used by tests only. Loaded by Node
// and Vite: relative imports with the `.ts` extension.
import { resolveBody } from "../../scenario.ts";
import { designScenarios } from "../index.ts";

export type ServedCall = { method: string; path: string; headers: Headers; body: string | undefined };
export type Served = (call: ServedCall) => Response | Promise<Response>;

/** `{id}` segments of an OpenAPI path template matched against a path: the parameters by name, or null when it is another path. */
export function matchTemplate(template: string, pathname: string): Record<string, string> | null {
  const want = template.split("/");
  const have = pathname.split("/");
  if (want.length !== have.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < want.length; i++) {
    const w = want[i] ?? "";
    const h = have[i] ?? "";
    if (w.startsWith("{") && w.endsWith("}")) params[w.slice(1, -1)] = decodeURIComponent(h);
    else if (w !== h) return null;
  }
  return params;
}

const problemResponse = (status: number): Response =>
  new Response(JSON.stringify({ type: "urn:lanekeeper:problem:not_found", title: "not_found", status, request_id: "req-test-1" }), { status, headers: { "content-type": "application/problem+json" } });

/**
 * Serve scenario `id`. `override` answers first and may return nothing to leave the call to the scenario. A call the scenario does
 * not know is a 404 problem.
 */
export function serveScenario(id: string, override?: (call: ServedCall) => Response | Promise<Response> | undefined): Served {
  const scenario = designScenarios[id];
  if (scenario === undefined) throw new Error(`no design scenario ${id}`);
  return (call) => {
    const overridden = override?.(call);
    if (overridden !== undefined) return overridden;
    const url = new URL(call.path, "http://localhost");
    for (const response of scenario.responses) {
      if (response.method !== call.method) continue;
      const params = matchTemplate(response.path, url.pathname);
      if (params === null) continue;
      let body: unknown;
      try {
        body = call.body === undefined ? undefined : JSON.parse(call.body);
      } catch {
        body = undefined;
      }
      const value = resolveBody(response, { params, query: url.searchParams, headers: call.headers, body });
      const status = response.status ?? 200;
      if (value === undefined) return new Response(null, { status });
      return new Response(JSON.stringify(value), { status, headers: { "content-type": status >= 400 ? "application/problem+json" : "application/json" } });
    }
    return problemResponse(404);
  };
}
