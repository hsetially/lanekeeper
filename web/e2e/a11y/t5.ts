// What the T5 cases share (the accessibility and layout specs of T5; the cases are in cases-t5-*.ts): changing one answer of a scenario
// while keeping the rest of it, a page that has lost its live stream, a server event, the steps that raise a pull request and reveal a
// masked value, and a list of pull requests the size of a busy swimlane.
import { expect } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import type { PrLink } from "../../src/api/generated/model/index.ts";
import { LINKS } from "../../src/mocks/design/prs/fixtures.ts";
import { FROZEN_NOW } from "../visual/render-references.mjs";
import type { Ctx } from "./helpers.ts";
import { modal } from "./helpers.ts";

type Json = Record<string, unknown>;

/** The `items` of a page of a list answer (an empty list when the answer has none). */
export function itemsOf(page: Json): Array<Record<string, unknown>> {
  const items = page["items"];
  return Array.isArray(items) ? (items as Array<Record<string, unknown>>) : [];
}

/** The scenario's own answer to the requests that match, with `change` applied to its JSON (the rest of the scenario stays as it is). */
export async function patchJson(page: Page, pattern: string | RegExp, change: (body: Json, url: URL) => Json | Promise<Json>): Promise<void> {
  await page.route(pattern, async (route: Route) => {
    // A request the page gave up (it moved on to another query before the answer came) has no answer to change: let it go.
    const response = await route.fetch().catch(() => null);
    if (response === null) return;
    if (!(response.headers()["content-type"] ?? "").includes("json")) {
      await route.fulfill({ response }).catch(() => undefined);
      return;
    }
    const body = await response.json().then((b: unknown) => b as Json).catch(() => null);
    if (body === null) {
      await route.fulfill({ response }).catch(() => undefined);
      return;
    }
    await route.fulfill({ response, json: await change(body, new URL(route.request().url())) }).catch(() => undefined);
  });
}

/** Answer the requests that match with this JSON (and status). */
export async function answerJson(page: Page, pattern: string | RegExp, json: unknown, status = 200): Promise<void> {
  await page.route(pattern, (route) => route.fulfill({ status, json }));
}

/** A server-sent event frame, as the harness writes it to every open stream. */
export const frame = (id: number, event: Record<string, unknown>): string => `id: ${id}\nevent: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;

/** The page loses its live stream and cannot get another: "Live updates are paused" (the stream is refused 503 for as long as the test runs). */
export async function goOffline(ctx: Pick<Ctx, "page" | "harness">): Promise<void> {
  await expect.poll(() => ctx.harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
  ctx.harness.refuseStreams(503, 100);
  ctx.harness.closeStreams();
  // The shell says "Live updates paused, retrying" once the stream has failed to come back (before that it says "Reconnecting").
  await expect(ctx.page.getByText(/live updates paused/i).first()).toBeVisible({ timeout: 30_000 });
}

/** The pull requests the way the hub lists them: one swimlane, one state or all, newest first, and a page of `limit`. */
export function listPrs(links: readonly PrLink[], url: URL): { items: PrLink[]; next: string | null } {
  const lane = url.searchParams.get("swimlane");
  const state = url.searchParams.get("state");
  const limit = Number(url.searchParams.get("limit") ?? "100");
  const cursor = Number(url.searchParams.get("cursor") ?? "0");
  const all = links.filter((l) => (lane === null || l.swimlane === lane) && (state === null || l.state === state)).sort((a, b) => b.id - a.id);
  const items = all.slice(cursor, cursor + limit);
  return { items, next: cursor + limit < all.length ? String(cursor + limit) : null };
}

/** Make the hub's list of pull requests these links (the scenario's own is replaced). */
export async function withPrs(page: Page, links: readonly PrLink[] = LINKS): Promise<void> {
  await page.route(/\/api\/v1\/prs(\?.*)?$/, (route) => route.fulfill({ json: listPrs(links, new URL(route.request().url())) }));
}

/** A busy swimlane: `count` links, newest first, a quarter of them merged and a quarter closed, every one for a file of its own. */
export function manyPrs(count: number): PrLink[] {
  const base = LINKS[0];
  if (base === undefined) throw new Error("no link to copy");
  return Array.from({ length: count }, (_, i): PrLink => {
    const n = count - i;
    return {
      ...base,
      id: 1000 + n,
      pr_number: 500 + n,
      pr_url: `https://github.com/csp-tenant/csp-tenant-data/pull/${500 + n}`,
      path: `tx-infinity-api/service-${String(n).padStart(3, "0")}/application-sit7.yml`,
      state: i % 4 === 2 ? "merged" : i % 4 === 3 ? "closed" : "open",
      created_at: FROZEN_NOW - (i + 1) * 17 * 60_000,
      closed_at: i % 4 >= 2 ? FROZEN_NOW - i * 9 * 60_000 : null,
    };
  });
}

// ---- The file page ----------------------------------------------------------------------------------------------------------------------
export const CORE_FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml";

/** Raise a pull request from the file page: the dialog, the target, the title (step 2) and, from step 3, the click that creates it. */
export async function raisePr(page: Page, toStep: 1 | 2 | 3 = 1): Promise<void> {
  await page.getByRole("button", { name: "Raise PR", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Raise pull request" })).toBeVisible();
  if (toStep >= 2) {
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page.getByRole("textbox", { name: "Title" })).toBeVisible();
  }
  if (toStep >= 3) await page.getByRole("button", { name: "Create PR" }).click();
}

/** The answer of `getPrJob`: the mock job 7 of the file scenarios, in a state, with anything else changed. */
export async function jobIs(page: Page, state: string, over: Json = {}): Promise<void> {
  await patchJson(page, /\/api\/v1\/pr-jobs\/\d+$/, (job) => ({ ...job, state, ...(state === "completed" ? {} : { pr_url: null }), ...over }));
}

// ---- Masked values ------------------------------------------------------------------------------------------------------------------------
export const REASON = "Teller login fails on sit7; checking the secret matches the vault entry (OPS-2291).";

/** The Reveal button of the first (or the n-th) masked value on the page, and the dialog it opens. */
export async function openReveal(page: Page, n = 0): Promise<void> {
  await page.getByRole("button", { name: /^Reveal value of / }).nth(n).click();
  await expect(modal(page).getByRole("heading", { name: "Reveal masked value?" }).or(modal(page).getByText("Reveal masked value?")).first()).toBeVisible();
}

/** Type the reason and press the dialog's button. */
export async function sendReveal(page: Page, reason = REASON): Promise<void> {
  await modal(page).getByRole("textbox", { name: /Reason/ }).fill(reason);
  await modal(page).getByRole("button", { name: "Reveal value", exact: true }).click();
}

/** Reveal the n-th masked value and wait until it is on the page. */
export async function reveal(page: Page, n = 0): Promise<void> {
  await openReveal(page, n);
  await sendReveal(page);
  await expect(page.getByRole("button", { name: /^Hide value of / })).toBeVisible();
}

/**
 * The masked core file with three secrets on consecutive lines (19, 20 and 21), each of which names its setting, so each has a Reveal.
 * Settings that sit side by side in a file are the usual case for secrets (a client id, a secret, a token), and the Reveal buttons are
 * small controls in rows of 18 px.
 */
export function withAdjacentSecrets(content: Json): Json {
  const text = content["text"];
  const masks = content["masks"];
  if (typeof text !== "string" || !Array.isArray(masks)) return content;
  const eol = "\r\n";
  const lines = text.split(eol);
  const encoder = new TextEncoder();
  const marked = [
    { line: 20, label: "    vaultToken: ", marker: "[REDACTED: 31 chars, generic_secret]", setting: "txInfinityOptions.remote-itm-teller.vaultToken" },
    { line: 21, label: "    refreshToken: ", marker: "[REDACTED: 18 chars, jwt]", setting: "txInfinityOptions.remote-itm-teller.refreshToken" },
  ];
  for (const m of marked) lines[m.line - 1] = `${m.label}${m.marker}`;
  const added = marked.map((m) => {
    const lineStart = lines.slice(0, m.line - 1).reduce((sum, l) => sum + encoder.encode(l).length + eol.length, 0);
    const start = lineStart + encoder.encode(m.label).length;
    return { line: m.line, start, end: start + encoder.encode(m.marker).length, setting: m.setting };
  });
  return { ...content, text: lines.join(eol), masks: [...(masks as unknown[]), ...added] };
}
