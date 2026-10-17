import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { ApiProblem, PROBLEM_URN_PREFIX, parseProblemBody, problemFromResponse, readBounded } from "./problem";

const jsonResponse = (body: unknown, status: number, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/problem+json", ...headers } });

describe("problemFromResponse", () => {
  it("reads the kind from the type, the title, the status and the request id", async () => {
    const p = await problemFromResponse(
      jsonResponse({ type: `${PROBLEM_URN_PREFIX}conflict`, title: "Conflict", status: 409, request_id: "req_abc123", current: "a".repeat(64) }, 409),
    );
    expect(p).toBeInstanceOf(ApiProblem);
    expect(p.status).toBe(409);
    expect(p.kind).toBe("conflict");
    expect(p.title).toBe("Conflict");
    expect(p.requestId).toBe("req_abc123");
    expect(p.body["current"]).toBe("a".repeat(64));
  });

  it("normalises kebab-case kinds so one table covers both spellings of the contract", async () => {
    const p = await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}access-pending`, title: "Access pending", status: 403 }, 403));
    expect(p.kind).toBe("access_pending");
    expect(p.rawKind).toBe("access-pending");
  });

  it("falls back to the request id header, then to nothing", async () => {
    const withHeader = await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}internal`, title: "x", status: 500 }, 500, { "x-request-id": "d1c7b8cb0c941473" }));
    expect(withHeader.requestId).toBe("d1c7b8cb0c941473");
    const none = await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}internal`, title: "x", status: 500 }, 500));
    expect(none.requestId).toBeUndefined();
  });

  it("refuses a request id that is not a short token", async () => {
    const p = await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}internal`, title: "x", status: 500, request_id: "<script>alert(1)</script>" }, 500));
    expect(p.requestId).toBeUndefined();
  });

  it("makes a problem of a body that is not one (an HTML error page from a proxy)", async () => {
    const response = new Response("<html><body>Bad gateway</body></html>", { status: 502, headers: { "content-type": "text/html" } });
    const p = await problemFromResponse(response);
    expect(p.status).toBe(502);
    expect(p.kind).toBe("unknown");
    expect(p.title).toBe("HTTP 502");
    expect(JSON.stringify(p.body)).not.toContain("Bad gateway");
  });

  it("makes a problem of an empty body", async () => {
    const p = await problemFromResponse(new Response(null, { status: 503 }));
    expect(p.status).toBe(503);
    expect(p.kind).toBe("unknown");
  });

  it("uses the status of the response, not the one in the body", async () => {
    const p = await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}forbidden`, title: "x", status: 200 }, 403));
    expect(p.status).toBe(403);
  });

  it("reads Retry-After in seconds and ignores nonsense", async () => {
    expect((await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}too_many_requests`, title: "x", status: 429 }, 429, { "retry-after": "17" }))).retryAfterSeconds).toBe(17);
    expect((await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}too_many_requests`, title: "x", status: 429 }, 429, { "retry-after": "soon" }))).retryAfterSeconds).toBeUndefined();
    expect((await problemFromResponse(jsonResponse({ type: `${PROBLEM_URN_PREFIX}too_many_requests`, title: "x", status: 429 }, 429, { "retry-after": "99999999" }))).retryAfterSeconds).toBe(3600);
  });

  it("does not read more than the cap of an endless body", async () => {
    const big = "x".repeat(200_000);
    const p = await problemFromResponse(new Response(`{"type":"${PROBLEM_URN_PREFIX}internal","title":"${big}","status":500}`, { status: 500, headers: { "content-type": "application/problem+json" } }));
    expect(p.kind).toBe("unknown");
  });

  it("carries no request data: the only inputs are the response", () => {
    const p = new ApiProblem({ status: 0, kind: "network", title: "Network error" });
    expect(Object.keys(p)).not.toContain("request");
    expect(Object.keys(p)).not.toContain("init");
  });
});

describe("readBounded", () => {
  it("stops at the byte limit and says so", async () => {
    const out = await readBounded(new Response("a".repeat(1000)), 100);
    expect(out.truncated).toBe(true);
    expect(out.text.length).toBeLessThanOrEqual(100);
  });
  it("returns everything under the limit", async () => {
    const out = await readBounded(new Response("hello"), 100);
    expect(out).toEqual({ text: "hello", truncated: false });
  });
});

describe("parseProblemBody (property)", () => {
  it("never throws and always yields a valid status and a non-empty title, for any text", () => {
    const alphabet = '{}[]":,.\\ \n\tabcdeulnrst0123456789<>/\u00e9\u202e\u0000';
    forAll(
      10_000,
      (rng) => ({ text: rng.string(alphabet, 60), status: 400 + rng.int(200) }),
      ({ text, status }) => {
        const p = parseProblemBody(text, status, new Headers());
        expect(p).toBeInstanceOf(ApiProblem);
        expect(p.status).toBe(status);
        expect(p.title.length).toBeGreaterThan(0);
        expect(p.kind).toMatch(/^[a-z][a-z0-9_]{0,63}$/);
      },
    );
  });
});
