// Steps the frame tables share: held and failed requests (`page.route`), a row opened by its text, a tooltip shown by hovering.
import type { Locator, Page } from "@playwright/test";
import { expect } from "../../fixtures/test";
import type { Env } from "./kit";
import { problemBody } from "../../../src/mocks/scenario.ts";

/** Never answer a request that matches: the screen stays in its loading state (the harness' own answer is never asked). */
export async function holdRequests(page: Page, glob: string | RegExp): Promise<void> {
  await page.route(glob, () => {
    // Left pending on purpose.
  });
}

const CODES: Record<number, string> = { 403: "forbidden", 404: "not_found", 409: "conflict", 500: "internal" };

/** Answer a request that matches with a problem+json of `status`, with a fixed request id. */
export async function failRequests(page: Page, glob: string | RegExp, status = 500, title = "Internal error"): Promise<void> {
  await page.route(glob, (route) =>
    route.fulfill({ status, contentType: "application/problem+json", headers: { "x-request-id": "req-mock-0001" }, body: JSON.stringify(problemBody(status, CODES[status] ?? "unavailable", title)) }),
  );
}

/** The row of a grid or table that holds this text. */
export function rowWith(page: Page, text: string | RegExp) {
  return page.getByRole("row", { name: text }).first();
}

export async function expectSkeleton(page: Page): Promise<void> {
  await expect(page.locator("[data-slot='skeleton'], [aria-busy='true']").first()).toBeVisible();
}

/** The hub refuses the event stream (a 503 and a retry in a second): after three failures the shell says "Live updates paused, retrying". */
export function pauseStream({ harness }: Env): Promise<void> {
  harness.refuseStreams(503, 99, { retryAfter: 1 });
  return Promise.resolve();
}

/** Wait for the shell's paused banner. Its countdown ("Next attempt in 7s") and the pill's are jittered: frames mask `pausedMasks`. */
export async function expectPaused(page: Page): Promise<void> {
  await expect(page.getByText("Live updates paused, retrying")).toBeVisible({ timeout: 30_000 });
}

/** The live regions of a paused frame: the countdown to the next attempt, which has a jitter. */
export const pausedMasks = (page: Page): Locator[] => [page.getByText(/Next attempt in/)];
