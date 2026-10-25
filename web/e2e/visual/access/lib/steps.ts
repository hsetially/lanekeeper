// The things a frame's state needs the spec to do, that more than one spec does: hold a request so that the page stays loading, make
// one fail, push an event the way the hub does, and wait for something specific to draw.
import type { Page } from "@playwright/test";
import type { Harness } from "../../../harness/serve.ts";
import { problemBody } from "../../../../src/mocks/scenario.ts";
import { meBody } from "../../../../src/mocks/me.ts";
import { MARCO, PRIYA } from "../../../../src/mocks/design/shell/index.ts";
import type { Role } from "../../../../src/api/generated/model/index.ts";

/** The text of one server-sent event, byte for byte as the hub writes it (`id: N` with a space). */
export const sseFrame = (id: number | null, event: Record<string, unknown>): string => `${id === null ? "" : `id: ${String(id)}\n`}event: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;

/** Push events to every open stream, and let the page act on them. */
export async function pushEvents(page: Page, harness: Harness, ...events: Array<[number, Record<string, unknown>]>): Promise<void> {
  harness.emit(events.map(([id, event]) => sseFrame(id, event)).join(""));
  await page.waitForTimeout(400);
}

/** Never answer a request that matches (a skeleton stays on screen until the test ends). */
export async function hold(page: Page, pattern: RegExp): Promise<void> {
  await page.route(pattern, () => {
    // Left unanswered on purpose: the request is pending, which is the loading state.
  });
}

/** Never answer a matching request of one method (the PUT of a token, while its GET is answered as the scenario says). */
export async function holdMethod(page: Page, pattern: RegExp, method: string): Promise<void> {
  await page.route(pattern, (route) => {
    if (route.request().method() !== method) return route.fallback();
    // Left unanswered on purpose: the request is pending, which is "verifying".
    return undefined;
  });
}

/** Answer a matching request with a problem, the way the hub does (`application/problem+json`, a request id). */
export async function fail(page: Page, pattern: RegExp, status = 500, title = "Internal Server Error"): Promise<void> {
  await page.route(pattern, (route) =>
    route.fulfill({
      status,
      contentType: "application/problem+json",
      body: JSON.stringify(problemBody(status, status === 500 ? "internal" : "error", title)),
    }),
  );
}

/** The address of an operation under /api/v1, with or without a query. */
export const api = (path: string): RegExp => new RegExp(`/api/v1${path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\?.*)?$`);

const CSRF_COOKIE = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

/**
 * Who is signed in, when the board's frame is a person the scenario is not (the File board's Editor frames: Marco Diaz, an Editor,
 * on the data of a scenario that is Priya Shah, an Admin). Only `/me` is answered differently; everything else is the scenario's.
 */
export async function signedInAs(page: Page, role: Role, who: "marco" | "priya" = "marco", over: Record<string, unknown> = {}): Promise<void> {
  const person = who === "marco" ? { id: MARCO, display_name: "Marco Diaz", email: "marco.diaz@example.test" } : { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" };
  await page.route(api("/me"), (route) =>
    route.fulfill({ status: 200, contentType: "application/json", headers: { "set-cookie": CSRF_COOKIE }, body: JSON.stringify(meBody(role, { ...person, ...over })) }),
  );
}

/** Let a matching request through, and change what it answers (a state the scenario has no variant for: a job half way, a file with no key). */
export async function rewrite(page: Page, pattern: RegExp, change: (body: Record<string, unknown>) => Record<string, unknown>): Promise<void> {
  await page.route(pattern, async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: change(body) });
  });
}
