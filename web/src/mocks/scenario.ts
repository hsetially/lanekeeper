// Mock scenarios (T1). A scenario is DATA: for each operation it answers, the status and the body. The e2e harness serves
// the scenario the test chose; the unit tests serve it through msw; src/mocks/mocks.test.ts checks every response against the
// OpenAPI schemas, so a mock cannot drift from the contract.
//
// This folder is loaded by Node as well as by Vite (the harness imports it directly, with type stripping): relative imports
// with a `.ts` extension, `import type` for every type, and nothing that is not erasable (no enum, no namespace).
import { http, HttpResponse, type RequestHandler } from "msw";

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export type MockContext = {
  /** Path parameters by name. */
  params: Record<string, string>;
  query: URLSearchParams;
  headers: Headers;
  /** The JSON body of the request, or undefined. */
  body: unknown;
};

export type MockResponse = {
  method: HttpMethod;
  /** The OpenAPI path template, for example `/api/v1/swimlanes/{id}`. */
  path: string;
  status?: number;
  headers?: Record<string, string>;
  /** A value, or a function of the request. A function needs a `sample` so the validator can call it. */
  body?: unknown;
  sample?: Partial<MockContext>;
  /** More requests to check the body against the contract with: the first, a middle and the last page of a list. */
  samples?: readonly Partial<MockContext>[];
};

export type Scenario = {
  id: string;
  description: string;
  responses: readonly MockResponse[];
};

/** `/api/v1/swimlanes/{id}` to msw's `*\/api/v1/swimlanes/:id`. */
export function mswPath(path: string): string {
  return `*${path.replace(/\{([A-Za-z_]+)\}/g, ":$1")}`;
}

function bodyOf(response: MockResponse, context: MockContext): unknown {
  const body = response.body;
  return typeof body === "function" ? (body as (c: MockContext) => unknown)(context) : body;
}

/** The value of a response for a given request, as the validator and the handlers both build it. */
export function resolveBody(response: MockResponse, context: MockContext): unknown {
  return bodyOf(response, context);
}

export function contextFor(response: MockResponse): MockContext {
  return { params: {}, query: new URLSearchParams(), headers: new Headers(), body: undefined, ...response.sample };
}

export function toHandlers(scenario: Scenario): RequestHandler[] {
  return scenario.responses.map((response) => {
    const make = http[response.method.toLowerCase() as "get" | "post" | "put" | "patch" | "delete"];
    return make(mswPath(response.path), async ({ request, params }) => {
      let json: unknown;
      try {
        json = request.headers.get("content-type")?.includes("json") ? await request.clone().json() : undefined;
      } catch {
        json = undefined;
      }
      const url = new URL(request.url);
      const body = bodyOf(response, {
        params: Object.fromEntries(Object.entries(params).map(([k, v]) => [k, String(v)])),
        query: url.searchParams,
        headers: request.headers,
        body: json,
      });
      const status = response.status ?? 200;
      const headers: Record<string, string> = { ...response.headers };
      // A success with an ETag is answered 304, with no body, when the request holds that ETag: the hub does the same for the
      // content operations, and it is what the app's own ETag cache (src/api/etag.ts) is for.
      const etag = Object.entries(headers).find(([name]) => name.toLowerCase() === "etag")?.[1];
      if (etag !== undefined && status < 300 && request.headers.get("if-none-match") === etag) return new HttpResponse(null, { status: 304, headers: { etag } });
      if (body === undefined) return new HttpResponse(null, { status, headers });
      headers["content-type"] ??= status >= 400 ? "application/problem+json" : "application/json";
      return new HttpResponse(JSON.stringify(body), { status, headers });
    });
  });
}

/** A problem body as the hub sends it (RFC 9457, with `type` as the discriminator). */
export function problemBody(status: number, code: string, title: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { type: `urn:lanekeeper:problem:${code}`, title, status, request_id: "req-mock-0001", ...extra };
}
