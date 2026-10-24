import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { parse } from "yaml";
import { apiFetch, apiCaches, NEVER_AUTO_RETRY_PATHS } from "./client";
import { ApiProblem } from "./problem";
import { onSessionSignal, type SessionSignal } from "./session-events";
import * as endpoints from "./generated/endpoints";

const TOKEN = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

type Call = { url: string; init: RequestInit & { headers: Headers } };
let calls: Call[];
let queue: Array<Response | Error | ((call: Call) => Promise<Response>)>;

function json(body: unknown, init: ResponseInit = {}): Response {
  const { headers, ...rest } = init;
  return new Response(JSON.stringify(body), { status: 200, ...rest, headers: { "content-type": "application/json", ...(headers as Record<string, string> | undefined) } });
}

/** A fetch that never answers but, like the real one, rejects when its signal aborts. */
const hangs = (call: Call): Promise<Response> =>
  new Promise<Response>((_, reject) => (call.init.signal as AbortSignal).addEventListener("abort", () => reject(new DOMException("aborted", "AbortError"))));

beforeEach(() => {
  calls = [];
  queue = [];
  apiCaches.clear();
  vi.stubGlobal("document", { cookie: `__Host-lk_csrf=${TOKEN}` });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit) => {
      const call: Call = { url, init: { ...init, headers: new Headers(init.headers) } };
      calls.push(call);
      const next = queue.shift();
      if (next === undefined) return json({ ok: true });
      if (next instanceof Error) throw next;
      if (typeof next === "function") return next(call);
      return next;
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("what every request carries", () => {
  it("sends a GET with no CSRF token and no Idempotency-Key", async () => {
    await apiFetch("/api/v1/me");
    const { init } = calls[0] as Call;
    expect(init.method).toBe("GET");
    expect(init.headers.get("x-csrf-token")).toBeNull();
    expect(init.headers.get("idempotency-key")).toBeNull();
  });

  it("uses the settings that make the hub see the real Origin and never a cached answer", async () => {
    await apiFetch("/api/v1/me");
    const { init } = calls[0] as Call;
    expect(init.credentials).toBe("same-origin");
    // 'cors' is what makes a same-origin POST carry its real Origin; 'same-origin' would send `Origin: null` under no-referrer.
    expect(init.mode).toBe("cors");
    expect(init.cache).toBe("no-store");
    expect(init.redirect).toBe("error");
    expect(init.referrerPolicy).toBe("no-referrer");
  });

  it.each(["POST", "PUT", "PATCH", "DELETE"])("sends X-CSRF-Token and a fresh Idempotency-Key on a %s", async (method) => {
    await apiFetch("/api/v1/x", { method });
    await apiFetch("/api/v1/x", { method });
    const [first, second] = calls as [Call, Call];
    expect(first.init.headers.get("x-csrf-token")).toBe(TOKEN);
    expect(second.init.headers.get("x-csrf-token")).toBe(TOKEN);
    const keys = [first, second].map((c) => c.init.headers.get("idempotency-key") ?? "");
    for (const key of keys) expect(key).toMatch(UUID);
    expect(keys[0]).not.toBe(keys[1]);
  });

  it("uses an Idempotency-Key the caller names, so one attempt keeps one key", async () => {
    await apiFetch("/api/v1/x", { method: "POST", idempotencyKey: "11111111-1111-4111-8111-111111111111" });
    expect((calls[0] as Call).init.headers.get("idempotency-key")).toBe("11111111-1111-4111-8111-111111111111");
  });

  it("does not let a caller put the CSRF token or the key in by hand and bypass the cookie", async () => {
    await apiFetch("/api/v1/x", { method: "POST", headers: { "X-CSRF-Token": "forged-by-a-caller-of-the-client-0000" } });
    expect((calls[0] as Call).init.headers.get("x-csrf-token")).toBe(TOKEN);
  });

  it("still sends a write when the cookie is missing and lets the hub answer 403", async () => {
    vi.stubGlobal("document", { cookie: "" });
    queue.push(json({ type: "urn:lanekeeper:problem:csrf", title: "Forbidden", status: 403 }, { status: 403, headers: { "content-type": "application/problem+json" } }));
    await expect(apiFetch("/api/v1/x", { method: "POST" })).rejects.toMatchObject({ status: 403, kind: "csrf" });
    expect((calls[0] as Call).init.headers.get("x-csrf-token")).toBeNull();
  });

  it("sends only to the hub's own paths", async () => {
    for (const bad of [
      "https://evil.example/api/v1/x",
      "//evil.example/api/v1/x",
      "/api/v1/../x",
      "/api/v1/swimlanes/../../admin/users",
      "/api/v1/swimlanes/%2e%2e/admin",
      "/api/v1/swimlanes/%2E%2E%2Fadmin",
      "/api/v1/swimlanes/a%2Fb",
      "/api/v1/swimlanes/a%5Cb",
      "/api/v1/swimlanes//x",
      "/api/v1/x#frag",
      "/other",
      "api/v1/x",
      "/api/v1/x\\y",
      "/api/v2/x",
      "javascript:alert(1)",
      "/auth/../etc",
      "/api/v1/x y",
      `/api/v1/${"a".repeat(3000)}`,
    ]) {
      await expect(apiFetch(bad), bad).rejects.toThrow(/own paths/);
    }
    expect(calls).toHaveLength(0);
    await apiFetch("/auth/logout", { method: "POST" });
    expect(calls).toHaveLength(1);
  });

  it("covers every write operation of the contract: each declares the two headers, and the generated function sends them", async () => {
    const doc = parse(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as {
      paths: Record<string, Record<string, { operationId: string; parameters?: Array<{ $ref?: string }> }>>;
    };
    const generated = endpoints as unknown as Record<string, (...args: unknown[]) => Promise<unknown>>;
    let writes = 0;
    let reads = 0;
    for (const [path, item] of Object.entries(doc.paths)) {
      for (const [method, operation] of Object.entries(item)) {
        if (!["get", "post", "put", "patch", "delete"].includes(method)) continue;
        const refs = (operation.parameters ?? []).map((p) => p.$ref?.split("/").pop());
        const fn = generated[operation.operationId];
        if (path === "/api/v1/events") continue; // the stream has its own client (T3)
        if (path === "/hooks/github") {
          // GitHub calls this one, never a browser. The generated function exists and the client refuses to send it.
          await expect((fn as (...a: unknown[]) => Promise<unknown>)("x"), "the webhook is not callable from the app").rejects.toThrow(/own paths/);
          continue;
        }
        expect(fn, `${operation.operationId} is generated`).toBeTypeOf("function");
        calls.length = 0;
        queue.length = 0;
        queue.push(new Response(null, { status: 204 }));
        const args = Array.from({ length: Math.max(0, (fn as (...a: unknown[]) => unknown).length - 1) }, () => "x");
        await (fn as (...a: unknown[]) => Promise<unknown>)(...args).catch(() => undefined);
        const call = calls[0];
        expect(call, `${operation.operationId} sent a request`).toBeDefined();
        const headers = (call as Call).init.headers;
        if (method === "get") {
          reads++;
          expect(headers.get("x-csrf-token"), `${operation.operationId} (GET) must not carry a CSRF token`).toBeNull();
          expect(headers.get("idempotency-key"), `${operation.operationId} (GET) must not carry a key`).toBeNull();
        } else {
          writes++;
          expect(refs, `${operation.operationId} declares the CSRF header`).toContain("CsrfToken");
          // Sign-out is the one write with no Idempotency-Key in the contract (nothing to deduplicate); the client sends one anyway.
          if (operation.operationId === "authLogout") expect(refs).not.toContain("IdempotencyKey");
          else expect(refs, `${operation.operationId} declares the Idempotency-Key header`).toContain("IdempotencyKey");
          expect(headers.get("x-csrf-token"), `${operation.operationId} sends the CSRF token`).toBe(TOKEN);
          expect(headers.get("idempotency-key") ?? "", `${operation.operationId} sends a key`).toMatch(UUID);
        }
      }
    }
    expect(writes).toBeGreaterThanOrEqual(40);
    expect(reads).toBeGreaterThanOrEqual(40);
  });
});

describe("responses", () => {
  it("returns parsed JSON", async () => {
    queue.push(json({ a: 1 }));
    await expect(apiFetch<{ a: number }>("/api/v1/x")).resolves.toEqual({ a: 1 });
  });

  it("returns nothing for 204", async () => {
    queue.push(new Response(null, { status: 204 }));
    await expect(apiFetch("/api/v1/x", { method: "DELETE" })).resolves.toBeUndefined();
  });

  it("returns a Blob for a binary answer", async () => {
    queue.push(new Response(new Uint8Array([1, 2, 3]), { status: 200, headers: { "content-type": "application/octet-stream" } }));
    const blob = await apiFetch<Blob>("/api/v1/blobs/abc");
    expect(blob).toBeInstanceOf(Blob);
    expect(blob.size).toBe(3);
  });

  it("turns a 2xx with a body that is not JSON into a problem, never into undefined data", async () => {
    queue.push(new Response("<html>", { status: 200, headers: { "content-type": "application/json" } }));
    await expect(apiFetch("/api/v1/x")).rejects.toMatchObject({ kind: "bad_response" });
  });

  it("throws an ApiProblem for a problem response, with its request id", async () => {
    queue.push(json({ type: "urn:lanekeeper:problem:not_found", title: "Not found", status: 404, request_id: "req-1" }, { status: 404, headers: { "content-type": "application/problem+json" } }));
    const error = await apiFetch("/api/v1/x").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiProblem);
    expect(error).toMatchObject({ status: 404, kind: "not_found", requestId: "req-1" });
  });

  it("does not put what it sent into the error: a token in the body never reaches the problem", async () => {
    const secret = "ghp_SUPERSECRETTOKENVALUE0123456789abcdef";
    queue.push(json({ type: "urn:lanekeeper:problem:token_rejected", title: "Token rejected", status: 422 }, { status: 422, headers: { "content-type": "application/problem+json" } }));
    const error = await apiFetch("/api/v1/me/github-token", { method: "PUT", body: JSON.stringify({ token: secret }), headers: { "content-type": "application/json" } }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiProblem);
    const p = error as ApiProblem;
    expect(JSON.stringify(p)).not.toContain(secret);
    expect(p.message).not.toContain(secret);
    expect(String(p.stack)).not.toContain(secret);
    expect(JSON.stringify(Object.getOwnPropertyDescriptors(p))).not.toContain(secret);
  });

  it("refuses a body over the size cap", async () => {
    queue.push(new Response("{}", { status: 200, headers: { "content-type": "application/json", "content-length": String(200 * 1024 * 1024) } }));
    await expect(apiFetch("/api/v1/x")).rejects.toMatchObject({ kind: "response_too_large" });
  });
});

describe("the session signals", () => {
  const seen: SessionSignal[] = [];
  let stop: () => void;
  beforeEach(() => {
    seen.length = 0;
    stop = onSessionSignal((s) => seen.push(s));
  });
  afterEach(() => stop());

  it("tells the app about a 401 and empties the ETag cache", async () => {
    apiCaches.etags.set("/api/v1/me", '"a"', "{}");
    queue.push(json({ type: "urn:lanekeeper:problem:unauthenticated", title: "Sign in", status: 401 }, { status: 401, headers: { "content-type": "application/problem+json" } }));
    await expect(apiFetch("/api/v1/me")).rejects.toMatchObject({ status: 401 });
    expect(seen).toEqual([{ kind: "unauthenticated" }]);
    expect(apiCaches.etags.size).toBe(0);
  });

  it("tells the app about a user with no role yet and about a disabled one, in either spelling", async () => {
    for (const type of ["access-pending", "access_pending"]) {
      queue.push(json({ type: `urn:lanekeeper:problem:${type}`, title: "x", status: 403 }, { status: 403, headers: { "content-type": "application/problem+json" } }));
      await apiFetch("/api/v1/me").catch(() => undefined);
    }
    queue.push(json({ type: "urn:lanekeeper:problem:access-disabled", title: "x", status: 403 }, { status: 403, headers: { "content-type": "application/problem+json" } }));
    await apiFetch("/api/v1/me").catch(() => undefined);
    expect(seen).toEqual([{ kind: "access_pending" }, { kind: "access_pending" }, { kind: "access_disabled" }]);
  });

  it("says nothing for an ordinary 403", async () => {
    queue.push(json({ type: "urn:lanekeeper:problem:insufficient-role", title: "x", status: 403 }, { status: 403, headers: { "content-type": "application/problem+json" } }));
    await apiFetch("/api/v1/x", { method: "POST" }).catch(() => undefined);
    expect(seen).toEqual([]);
  });
});

describe("ETag revalidation", () => {
  const etagResponse = (body: unknown, etag: string) => json(body, { headers: { etag } });

  it("stores a GET answer that has an ETag and sends If-None-Match next time, using the kept body on 304", async () => {
    queue.push(etagResponse({ v: 1 }, '"h1"'));
    await expect(apiFetch("/api/v1/swimlanes/a/files/content?path=x")).resolves.toEqual({ v: 1 });
    queue.push(new Response(null, { status: 304, headers: { etag: '"h1"' } }));
    await expect(apiFetch("/api/v1/swimlanes/a/files/content?path=x")).resolves.toEqual({ v: 1 });
    expect((calls[0] as Call).init.headers.get("if-none-match")).toBeNull();
    expect((calls[1] as Call).init.headers.get("if-none-match")).toBe('"h1"');
  });

  it("gives each caller its own copy of the kept body", async () => {
    queue.push(etagResponse({ v: 1 }, '"h1"'));
    await apiFetch("/api/v1/c");
    queue.push(new Response(null, { status: 304 }));
    const a = await apiFetch<{ v: number }>("/api/v1/c");
    a.v = 99;
    queue.push(new Response(null, { status: 304 }));
    await expect(apiFetch("/api/v1/c")).resolves.toEqual({ v: 1 });
  });

  it("replaces the kept body when the answer changes", async () => {
    queue.push(etagResponse({ v: 1 }, '"h1"'));
    await apiFetch("/api/v1/c");
    queue.push(etagResponse({ v: 2 }, '"h2"'));
    await expect(apiFetch("/api/v1/c")).resolves.toEqual({ v: 2 });
    queue.push(new Response(null, { status: 304 }));
    await expect(apiFetch("/api/v1/c")).resolves.toEqual({ v: 2 });
    expect((calls[2] as Call).init.headers.get("if-none-match")).toBe('"h2"');
  });

  it("never stores or sends for a write, and does not store an answer without an ETag", async () => {
    queue.push(etagResponse({ w: 1 }, '"w"'));
    await apiFetch("/api/v1/c", { method: "POST" });
    queue.push(json({ n: 1 }));
    await apiFetch("/api/v1/d");
    await apiFetch("/api/v1/c");
    await apiFetch("/api/v1/d");
    expect((calls[2] as Call).init.headers.get("if-none-match")).toBeNull();
    expect((calls[3] as Call).init.headers.get("if-none-match")).toBeNull();
  });

  it("does not store a binary answer", async () => {
    queue.push(new Response(new Uint8Array([1]), { status: 200, headers: { "content-type": "application/octet-stream", etag: '"b"' } }));
    await apiFetch("/api/v1/blobs/x");
    await apiFetch("/api/v1/blobs/x");
    expect((calls[1] as Call).init.headers.get("if-none-match")).toBeNull();
  });

  it("does not keep the answer of an operation that returns a secret", async () => {
    for (const path of ["/api/v1/me/mcp-tokens", "/api/v1/me/github-token"]) {
      queue.push(etagResponse({ token: "secret" }, '"s"'));
      await apiFetch(path);
      await apiFetch(path);
      expect((calls.at(-1) as Call).init.headers.get("if-none-match"), path).toBeNull();
    }
  });

  it("turns a 304 with nothing kept into a problem", async () => {
    queue.push(new Response(null, { status: 304 }));
    await expect(apiFetch("/api/v1/c")).rejects.toMatchObject({ kind: "bad_response" });
  });
});

describe("failures with no response", () => {
  it("turns a network error into a problem with status 0", async () => {
    queue.push(new TypeError("Failed to fetch"));
    await expect(apiFetch("/api/v1/x")).rejects.toMatchObject({ status: 0, kind: "network" });
  });

  it("times out", async () => {
    vi.useFakeTimers();
    queue.push(hangs);
    const pending = apiFetch("/api/v1/x", { timeoutMs: 50 }).catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(60);
    expect(await pending).toMatchObject({ status: 0, kind: "timeout" });
  });

  it("reports a caller's abort as aborted, not as a failure to retry", async () => {
    const controller = new AbortController();
    queue.push(hangs);
    const pending = apiFetch("/api/v1/x", { signal: controller.signal }).catch((e: unknown) => e);
    controller.abort();
    expect(await pending).toMatchObject({ status: 0, kind: "aborted" });
  });

  it("retries a write once after a network error, with the same Idempotency-Key", async () => {
    queue.push(new TypeError("Failed to fetch"), json({ ok: true }));
    await expect(apiFetch("/api/v1/swimlanes/a/files", { method: "PUT" })).resolves.toEqual({ ok: true });
    expect(calls).toHaveLength(2);
    expect((calls[0] as Call).init.headers.get("idempotency-key")).toBe((calls[1] as Call).init.headers.get("idempotency-key"));
  });

  it("does not retry a write twice", async () => {
    queue.push(new TypeError("1"), new TypeError("2"), json({ ok: true }));
    await expect(apiFetch("/api/v1/swimlanes/a/files", { method: "PUT" })).rejects.toMatchObject({ kind: "network" });
    expect(calls).toHaveLength(2);
  });

  it("does not retry a GET itself: the query layer decides", async () => {
    queue.push(new TypeError("1"), json({ ok: true }));
    await expect(apiFetch("/api/v1/x")).rejects.toMatchObject({ kind: "network" });
    expect(calls).toHaveLength(1);
  });

  it.each([
    ["revealValue", "/api/v1/swimlanes/a/files/reveal", "POST"],
    ["putGithubToken", "/api/v1/me/github-token", "PUT"],
    ["createMcpToken", "/api/v1/me/mcp-tokens", "POST"],
    ["adminIssueJoinToken", "/api/v1/admin/agents/sit1/join-token", "POST"],
    ["adminPutNotification", "/api/v1/admin/notifications/3", "PUT"],
  ])("never retries %s: a second send of a secret, or a second audited reveal", async (_name, path, method) => {
    queue.push(new TypeError("1"), json({ ok: true }));
    await expect(apiFetch(path, { method })).rejects.toMatchObject({ kind: "network" });
    expect(calls).toHaveLength(1);
  });

  it("names the operations that must never be retried in one list that the contract confirms", () => {
    const doc = parse(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as {
      paths: Record<string, Record<string, { operationId?: string }>>;
    };
    const wanted = ["revealValue", "putGithubToken", "createMcpToken", "adminIssueJoinToken", "adminPutNotification"];
    const found = new Map<string, string>();
    for (const [path, item] of Object.entries(doc.paths)) for (const operation of Object.values(item)) if (operation.operationId && wanted.includes(operation.operationId)) found.set(operation.operationId, path);
    expect([...found.keys()].sort()).toEqual([...wanted].sort());
    for (const path of found.values()) {
      expect(
        NEVER_AUTO_RETRY_PATHS.some((re) => re.test(path.replace(/\{[^}]+\}/g, "x1"))),
        `${path} is covered by NEVER_AUTO_RETRY_PATHS`,
      ).toBe(true);
    }
  });
});
