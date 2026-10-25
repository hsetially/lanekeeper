// What the T6 cases share (the accessibility and layout specs of T6; the cases are in cases-t6-*.ts): the addresses and the API paths
// of the swimlane the T6 scenarios are drawn on (sitb), a refusal with the headers a hub sends, a tooltip opened with the pointer, the
// tree of the Services tab, the steps that open the notify and restart dialogs, a file written (the "Saved" banner that holds the
// buttons), and the address of the "As served" page.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import { FROZEN_NOW } from "../visual/render-references.mjs";
import type { Ctx } from "./helpers.ts";
import { modal } from "./helpers.ts";
import { frame, goOffline, itemsOf, patchJson } from "./t5.ts";

export { frame, goOffline, itemsOf, patchJson };
export { FROZEN_NOW };

// ---- Addresses and API paths -----------------------------------------------------------------------------------------------------------
export const SERVICES_URL = "/swimlanes/sitb/services";
export const CORE_PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
export const CORE_FILE = `/swimlanes/sitb/files?path=${encodeURIComponent(CORE_PATH)}`;

export const API = {
  detail: /\/api\/v1\/swimlanes\/sitb$/,
  tree: /\/api\/v1\/swimlanes\/sitb\/tree/,
  notify: /\/api\/v1\/swimlanes\/sitb\/notify$/,
  restart: /\/api\/v1\/swimlanes\/sitb\/restart$/,
  configServerOffer: /\/api\/v1\/swimlanes\/sitb\/config-server-restart$/,
  restartOffer: /\/api\/v1\/swimlanes\/sitb\/restart-offer(\?.*)?$/,
  findings: /\/api\/v1\/findings(\?.*)?$/,
  served: /\/api\/v1\/swimlanes\/sitb\/served(\?.*)?$/,
  me: /\/api\/v1\/me$/,
} as const;

const NOW = FROZEN_NOW;

// ---- Answers -----------------------------------------------------------------------------------------------------------------------------
/** Refuse the requests that match, as a hub does: a problem+json with a request id (and any more headers, a Retry-After). */
export async function refuse(page: Page, pattern: string | RegExp, status: number, title: string, headers: Record<string, string> = {}): Promise<void> {
  await page.route(pattern, (route) =>
    route.fulfill({
      status,
      contentType: "application/problem+json",
      headers: { "x-request-id": "req-a11y-0001", ...headers },
      body: JSON.stringify({ type: "urn:lanekeeper:problem:test", title, status, request_id: "req-a11y-0001" }),
    }),
  );
}

/** The request never reaches the hub: the connection fails. */
export async function dropped(page: Page, pattern: string | RegExp): Promise<void> {
  await page.route(pattern, (route) => route.abort("failed"));
}

/** Let the first `count` requests that match through and refuse the next ones (the second restart of a run, for instance). */
export async function refuseAfter(page: Page, pattern: string | RegExp, count: number, status: number, title: string): Promise<void> {
  let seen = 0;
  await page.route(pattern, (route) => {
    seen += 1;
    if (seen <= count) return route.fallback();
    return route.fulfill({
      status,
      contentType: "application/problem+json",
      headers: { "x-request-id": "req-a11y-0001" },
      body: JSON.stringify({ type: "urn:lanekeeper:problem:test", title, status, request_id: "req-a11y-0001" }),
    });
  });
}

/** Change what the hub says about the swimlane sitb (the detail the Services tab, the banner and the buttons read). */
export async function patchLane(page: Page, change: (lane: Record<string, unknown>) => Record<string, unknown>): Promise<void> {
  await patchJson(page, API.detail, (lane) => change(lane));
}

/** A service of sitb in the shape of `ServiceState`. */
export function svc(name: string, over: Record<string, unknown> = {}): Record<string, unknown> {
  return { service: { namespace: "sitb", name }, pickup: "live", pending_since: null, live_by: null, notifications_enabled: true, cache_ttl_secs: 300, pending_files: [], ...over };
}

/** `count` files that services of sitb wait on, one path each, in folders (for a request that holds more than the 100 paths it can). */
export function manyFiles(count: number): string[] {
  return Array.from({ length: count }, (_, i) => `tx-infinity-api/service-${String(i).padStart(3, "0")}/application-sit7.yml`);
}

export { NOW };

// ---- The pickup chips ------------------------------------------------------------------------------------------------------------------
export const chips = (page: Page) => page.locator("[data-slot='pickup-chip']");

/** Move the pointer onto a chip and wait for its tooltip: the sentence that says when the change takes effect. */
export async function showTip(page: Page, chip: ReturnType<typeof chips>, text?: RegExp | string): Promise<void> {
  await chip.scrollIntoViewIfNeeded();
  await chip.hover();
  const tip = page.getByRole("tooltip").first();
  await expect(tip).toBeVisible();
  if (text !== undefined) await expect(tip).toContainText(text);
}

/** Open a folder of the file tree of the Services tab (its chevron) and wait for its rows. */
export async function openFolder(page: Page, name: string, firstChild: string): Promise<void> {
  await page.getByRole("button", { name: `Open ${name}` }).click();
  await expect(page.getByRole("tree").getByText(firstChild, { exact: true }).first()).toBeVisible();
}

// ---- The Services tab ---------------------------------------------------------------------------------------------------------------------
export const notifyButton = (page: Page) => page.getByRole("button", { name: "Notify services" }).first();
export const bannerRestart = (page: Page) => page.getByRole("button", { name: /^Restart config-server \(type its name to confirm\)$/ });

export async function openNotify(page: Page): Promise<void> {
  await notifyButton(page).click();
  await expect(modal(page).getByText(/Notify services in sitb\?|Notification|Couldn't/).first()).toBeVisible();
}

/** The dialog of "Notify services" and its confirmation: it starts in the confirm step (the files and the services). */
export async function confirmNotify(page: Page): Promise<void> {
  await modal(page).getByRole("button", { name: "Notify services", exact: true }).click();
}

export async function openConfigServerRestart(page: Page): Promise<void> {
  await bannerRestart(page).click();
}

/** Type what a type-to-confirm dialog asks for. */
export async function typeToConfirm(page: Page, text: string): Promise<void> {
  await modal(page).getByRole("textbox").fill(text);
}

// ---- The file page ------------------------------------------------------------------------------------------------------------------------
/** Edit the file, make a change and save it: the page says "Saved" and offers what the file's pickup state calls for. */
export async function saveFile(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(page.locator(".monaco-editor").first()).toBeVisible({ timeout: 20_000 });
  const box = await page.locator(".monaco-editor").first().boundingBox();
  if (!box) throw new Error("no editor box");
  await page.mouse.click(box.x + 200, box.y + 60);
  await page.keyboard.press("Control+End");
  await page.keyboard.type("# note");
  const send = page.getByRole("button", { name: /^(Save to sitb NFS|Submit for approval)$/ });
  await expect(send).toBeEnabled();
  await send.click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await page.getByRole("alertdialog").getByRole("button").last().click();
  await expect(page.getByText(/Saved to sitb NFS/)).toBeVisible();
}

/** The notify and restart buttons of the "Saved" banner. */
export const bannerButtons = (page: Page) => page.getByRole("button", { name: /^(Notify services|Restart services|Restart config-server)/ });

// ---- As served ---------------------------------------------------------------------------------------------------------------------------
export const SERVED_BASE = "/swimlanes/sitb/served";
const FILE = "tx-infinity-api/connection-pool.yml";

export type Asked = { file?: string; application?: string; tenant?: string; channel?: string };

/** The address of the page for a selection (everything but the file defaults to what the scenarios answer for). */
export function servedUrl(asked: Asked = {}): string {
  const q = new URLSearchParams();
  q.set("application", asked.application ?? "tx-infinity-core");
  q.set("tenant", asked.tenant ?? "sit7");
  if (asked.channel !== undefined) q.set("channel", asked.channel);
  q.set("file", asked.file ?? FILE);
  return `${SERVED_BASE}?${q.toString()}`;
}

/** The answer of `getServedFile`, with `change` applied (the scenario's own answer stays as it is otherwise). */
export async function patchServed(page: Page, change: (served: Record<string, unknown>) => Record<string, unknown>): Promise<void> {
  await patchJson(page, API.served, (served) => change(served));
}

/** The lines of a long served text (a thousand of them, of ordinary settings). */
export function longText(lines: number): string {
  return Array.from({ length: lines }, (_, i) => `setting.number${String(i).padStart(4, "0")}: value-${i}`).join("\n");
}

export type { Ctx };
