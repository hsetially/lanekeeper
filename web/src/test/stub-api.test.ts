import { afterEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { json, problem, routes, stubApi, TOKEN } from "./app";

// The fake hub of the component tests: `fetch` answered by a function of the test, every call kept. It used to be written out in the
// test helpers of the file view, the pull requests and the notify pieces (and the served page), each with a copy of `routes`.

afterEach(() => {
  vi.unstubAllGlobals();
  apiCaches.clear();
});

describe("stubApi", () => {
  it("answers fetch with the test's function and keeps every call, with the method upper-cased", async () => {
    const calls = stubApi((call) => json({ path: call.path, method: call.method }));
    const answer = await fetch("/api/v1/x?y=1", { method: "put", body: "{\"a\":1}", headers: { "x-test": "1" } });
    expect(await answer.json()).toEqual({ path: "/api/v1/x?y=1", method: "PUT" });
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "PUT", path: "/api/v1/x?y=1", body: "{\"a\":1}" });
    expect(calls[0]?.headers.get("x-test")).toBe("1");
  });

  it("reads a GET with no body as undefined, and a multipart body as the fields by name with a file as file:name:size", async () => {
    const calls = stubApi(() => json({}));
    await fetch("/api/v1/x");
    const form = new FormData();
    form.set("path", "docs/a.md");
    form.set("file", new File(["hello"], "a.md"));
    await fetch("/api/v1/docs", { method: "POST", body: form });
    expect(calls[0]?.body).toBeUndefined();
    expect(JSON.parse(calls[1]?.body ?? "{}")).toEqual({ path: "docs/a.md", file: "file:a.md:5" });
  });

  it("gives the client the CSRF cookie it reads, and starts with an empty answer cache", () => {
    apiCaches.etags.set("/api/v1/old", "\"1\"", "{}");
    stubApi(() => json({}));
    expect(document.cookie).toBe(`__Host-lk_csrf=${TOKEN}`);
    expect(apiCaches.etags.get("/api/v1/old")).toBeUndefined();
  });
});

describe("routes", () => {
  it("answers by 'METHOD /path' with the handler's value as a 200, ignoring the query", async () => {
    const api = routes({ "GET /api/v1/a": () => ({ ok: true }) });
    const response = await api({ method: "GET", path: "/api/v1/a?z=1", headers: new Headers(), body: undefined });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ ok: true });
  });

  it("passes a Response of the handler's through, and answers 404 for what the table does not have", async () => {
    const api = routes({ "POST /api/v1/a": () => problem(409, "conflict") });
    const conflict = await api({ method: "POST", path: "/api/v1/a", headers: new Headers(), body: undefined });
    expect(conflict.status).toBe(409);
    const missing = await api({ method: "GET", path: "/api/v1/a", headers: new Headers(), body: undefined });
    expect(missing.status).toBe(404);
  });
});
