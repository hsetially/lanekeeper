// Hostile text for the layout specs: what a name, a path, a message or a setting value can be when a person, not the design, chose it.
// Every string here is something the data of a real swimlane could hold (a name from a user, a path from NFS, a line from a file).
import type { Page } from "@playwright/test";

export const HOSTILE: readonly string[] = [
  // One long unbroken word: no place to wrap, so it has to be cut or scrolled, never allowed to push the page.
  `${"an-extremely-long-name-with-no-break-opportunity-".repeat(8)}end`,
  // Long, with ordinary words: it wraps, or truncates, and does not make a row taller than the row height.
  "A very long display name that goes on and on, with many ordinary words in it, longer than any column the design drew. ".repeat(2).trim(),
  // Right to left (Hebrew and Arabic) with a left to right word in the middle.
  "שלום עולם مرحبا بالعالم sitb שלום",
  // A right-to-left override: "invoice" then the rest reversed, the classic spoof.
  "invoice‮gnp.yml",
  // Isolates and marks, unpaired.
  "⁦sit⁩⁧b⁩‎‏",
  // Zero-width characters inside a word.
  "pay​ments‍-core﻿.yml⁠",
  // Combining marks stacked (a "zalgo" string): tall, and no wider than its letters.
  `Z${"̖̗̘̙̕".repeat(12)}algo`,
  // Emoji with joiners.
  "👩‍👩‍👧‍👦".repeat(12),
  // A control character and a tab in a name.
  "tab\there\u0007bell",
  // A deep path with long segments.
  `${Array.from({ length: 12 }, (_, i) => `segment-${i}-with-a-long-name`).join("/")}/file.yml`,
];

export const pick = (i: number): string => HOSTILE[i % HOSTILE.length] ?? "x";

/** Replace the string value of every one of `keys`, at any depth of a JSON body, by the next hostile string. */
export function hostileKeys(body: unknown, keys: readonly string[], counter = { n: 0 }): unknown {
  if (Array.isArray(body)) return body.map((x) => hostileKeys(x, keys, counter));
  if (body !== null && typeof body === "object") {
    return Object.fromEntries(
      Object.entries(body as Record<string, unknown>).map(([k, v]) => {
        if (keys.includes(k) && typeof v === "string") return [k, pick(counter.n++)];
        if (keys.includes(k) && Array.isArray(v) && v.every((x) => typeof x === "string")) return [k, v.map(() => pick(counter.n++))];
        return [k, hostileKeys(v, keys, counter)];
      }),
    );
  }
  return body;
}

/** Answer the requests that match with the real answer, in which `keys` are hostile. */
export async function makeHostile(page: Page, pattern: RegExp, keys: readonly string[]): Promise<void> {
  await page.route(pattern, async (route) => {
    const response = await route.fetch();
    const type = response.headers()["content-type"] ?? "";
    if (!type.includes("json")) {
      await route.fulfill({ response });
      return;
    }
    await route.fulfill({ response, json: hostileKeys(await response.json(), keys) });
  });
}

/** The markers SafeText drew for characters that must not be shown as themselves, with where they are. */
export async function markers(page: Page): Promise<Array<{ label: string; width: number; height: number; visible: boolean }>> {
  return page.evaluate(() =>
    [...document.querySelectorAll("[data-slot='safe-text'] span[role='img'][aria-label^='U+']")].map((el) => {
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      return { label: el.getAttribute("aria-label") ?? "", width: r.width, height: r.height, visible: r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none" };
    }),
  );
}
