// What the tests of the Admin sections share: the clock the design's relative times are drawn against, opening a section on the whole
// app, finding a row of a grid, and a response that can be held back. Test code only: nothing outside the tests imports it.
import { screen, within } from "@testing-library/react";
import { afterEach, beforeEach, vi } from "vitest";
import { renderApp } from "../../../test/app.tsx";
import { NOW } from "./fixtures.ts";
import { worldApi, type World } from "./world.ts";

/** Only `Date` is faked, at the design's frozen clock, so timers and promises still run. Call it at the top of a test file. */
export function installAdminClock(): void {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    Reflect.deleteProperty(document, "cookie");
  });
}

/** The first screen of a run loads the page's chunk, which takes longer than the default wait. */
export const FIRST = { timeout: 8000 };

export async function openAdmin(world: World, url: string) {
  const app = renderApp(url, worldApi(world));
  await screen.findByRole("heading", { level: 1, name: "Admin" }, FIRST);
  return app;
}

/** The row of a grid that holds `text`. */
export async function rowIn(grid: HTMLElement, text: string | RegExp): Promise<HTMLElement> {
  const cell = await within(grid).findByText(text);
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${String(text)}`);
  return row;
}

/** A response that the test lets go when it chooses. */
export function holdUntil(): { gate: Promise<void>; release: () => void } {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { gate, release };
}

export const writesOf = <T extends { method: string }>(calls: readonly T[]): T[] => calls.filter((c) => c.method !== "GET");
