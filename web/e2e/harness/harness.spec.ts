// The harness is test infrastructure that other specs trust, so it is tested: it proxies the hub's header code untouched, answers
// the mock scenario, refuses a write the way the hub's CSRF layer would, and writes raw SSE frames byte for byte.
import { readFileSync } from "node:fs";
import { expect, test } from "../fixtures/test";

const golden = (name: string): string => readFileSync(new URL(`../fixtures/sse-golden/${name}`, import.meta.url), "utf8");

async function readFor(response: Response, ms: number): Promise<string> {
  const reader = response.body?.getReader();
  if (!reader) return "";
  const decoder = new TextDecoder();
  let text = "";
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const result = await Promise.race([reader.read(), new Promise<{ done: true; value?: undefined }>((resolve) => setTimeout(() => resolve({ done: true }), Math.max(1, deadline - Date.now())))]);
    if (result.done) break;
    text += decoder.decode(result.value, { stream: true });
  }
  await reader.cancel().catch(() => undefined);
  return text;
}

test.describe("harness", () => {
  test("passes the hub's page through untouched: production CSP header and a nonce in the body", async ({ harness }) => {
    const response = await fetch(`${harness.url}/swimlanes`);
    const csp = response.headers.get("content-security-policy") ?? "";
    expect(csp).toContain("require-trusted-types-for 'script'");
    const nonce = /'nonce-([0-9a-f]{32})'/.exec(csp)?.[1];
    const body = await response.text();
    expect(nonce).toBeDefined();
    expect(body).toContain(`nonce="${nonce}"`);
    expect(body).not.toContain("__CSP_NONCE__");
  });

  test("answers the scenario, switches it, and says so loudly for an operation it does not know", async ({ harness }) => {
    harness.setScenario("editor");
    expect(((await (await fetch(`${harness.url}/api/v1/me`)).json()) as { role: string }).role).toBe("editor");
    harness.setScenario("signed-out");
    const denied = await fetch(`${harness.url}/api/v1/me`);
    expect(denied.status).toBe(401);
    expect(denied.headers.get("content-type")).toBe("application/problem+json");
    const unknown = await fetch(`${harness.url}/api/v1/swimlanes`);
    expect(unknown.status).toBe(501);
    expect(harness.requests().some((r) => r.unmocked === true)).toBe(true);
  });

  test("applies the hub's CSRF rules to a write that carries a session", async ({ harness }) => {
    const cookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0";
    const post = (headers: Record<string, string>) => fetch(`${harness.url}/auth/logout`, { method: "POST", headers: { cookie, ...headers } });
    expect((await post({})).status).toBe(403);
    expect((await post({ origin: harness.url })).status).toBe(403);
    expect((await post({ origin: "https://evil.example", "x-csrf-token": "mockMockMockMockMockMockMockMockMockMo0" })).status).toBe(403);
    expect((await post({ origin: harness.url, "x-csrf-token": "another-token-another-token-another-token" })).status).toBe(403);
    expect((await post({ origin: harness.url, "x-csrf-token": "mockMockMockMockMockMockMockMockMockMo0" })).status).toBe(204);
    // No session cookie: nothing to protect, the route's own guard answers (the hub's layer lets it through).
    expect((await fetch(`${harness.url}/auth/logout`, { method: "POST" })).status).toBe(204);
  });

  test("writes the golden SSE frames byte for byte, with the first frame and heartbeats", async ({ harness }) => {
    const response = await fetch(`${harness.url}/api/v1/events`, { headers: { accept: "text/event-stream", "last-event-id": "40" } });
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/event-stream");
    await expect.poll(() => harness.streamCount()).toBe(1);
    for (const name of ["heartbeat.txt", "file-observed.txt", "findings-changed.txt", "resync.txt", "final-retry.txt"]) harness.emit(golden(name));
    const text = await readFor(response, 800);
    expect(text).toBe(golden("first-frame.txt") + ["heartbeat.txt", "file-observed.txt", "findings-changed.txt", "resync.txt", "final-retry.txt"].map(golden).join(""));
    const log = harness.requests().find((r) => r.kind === "stream");
    expect(log?.lastEventId).toBe("40");
  });

  test("allows five streams for a user and refuses the sixth with 429, as the hub does", async ({ harness }) => {
    const open: Response[] = [];
    for (let i = 0; i < 5; i++) open.push(await fetch(`${harness.url}/api/v1/events`));
    const sixth = await fetch(`${harness.url}/api/v1/events`);
    expect(sixth.status).toBe(429);
    expect(sixth.headers.get("retry-after")).toBe("3");
    harness.closeStreams();
    await Promise.all(open.map(async (r) => r.body?.cancel().catch(() => undefined)));
    expect(harness.streamCount()).toBe(0);
  });

  test("refuses the next stream requests with the status the test names, then streams again", async ({ harness }) => {
    harness.refuseStreams(503, 2);
    expect((await fetch(`${harness.url}/api/v1/events`)).status).toBe(503);
    expect((await fetch(`${harness.url}/api/v1/events`)).status).toBe(503);
    const open = await fetch(`${harness.url}/api/v1/events`);
    expect(open.status).toBe(200);
    harness.refuseStreams(429, 1, { retryAfter: 7 });
    const busy = await fetch(`${harness.url}/api/v1/events`);
    expect(busy.status).toBe(429);
    expect(busy.headers.get("retry-after")).toBe("7");
    const refused = harness.requests().filter((r) => r.kind === "stream" && r.status !== 200);
    expect(refused.map((r) => r.status)).toEqual([503, 503, 429]);
    harness.closeStreams();
    await open.body?.cancel().catch(() => undefined);
  });
});
