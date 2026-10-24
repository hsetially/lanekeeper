import { act, configure, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { installAdminClock, rowIn } from "@/mocks/design/admin-t5/harness";
import { newT5World, t5Api, type T5World } from "@/mocks/design/admin-t5/world";
import { renderApp } from "@/test/app";
import { FakeHub } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// "Live updates paused" (States U rows) on the four sections of task T5: the stream is down for three attempts in a row. What is on
// screen stays, the shell says once that updates are paused, and every write is off with the reason (unavailable offline) instead of
// failing when it is tried. The dry run is a PUT as well, so it is off too.

installAdminClock();
configure({ asyncUtilTimeout: 15_000 });

let world: T5World;
let hub: FakeHub;
beforeEach(() => {
  hub = new FakeHub();
  world = newT5World();
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const fastTimers = {
  setTimeout: (run: () => void, ms: number) => globalThis.setTimeout(run, Math.min(ms, 5)),
  clearTimeout: (handle: ReturnType<typeof setTimeout>) => {
    globalThis.clearTimeout(handle);
  },
};

async function bringDown(url: string) {
  const app = renderApp(url, t5Api(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, timers: fastTimers, onlineTarget: new EventTarget() } } });
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 25_000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  for (let i = 0; i < 400; i++) hub.respond(503);
  await act(async () => {
    hub.last.fail();
    await Promise.resolve();
  });
  await screen.findByText("Live updates paused, retrying", {}, { timeout: 25_000 });
  return app;
}

const unavailable = (button: HTMLElement) => {
  expect(button).toHaveAttribute("aria-disabled", "true");
  expect(button).toHaveAccessibleDescription("Unavailable offline");
};

describe("when live updates are paused", () => {
  it("keeps the sentinels on screen, says once that updates are paused, and has no write to turn off", async () => {
    await bringDown("/admin/sentinels");
    expect(await screen.findByRole("grid", { name: "Sentinels" })).toBeInTheDocument();
    await rowIn(await screen.findByRole("grid", { name: "Sentinels" }), "nfs-sit-01");
    expect(screen.getAllByText(/Live updates paused/)).toHaveLength(1);
  }, 60_000);

  it("turns adding, changing and deleting an OS Login mapping off, and keeps the mappings on screen", async () => {
    await bringDown("/admin/oslogin");
    unavailable(await screen.findByRole("button", { name: "Add mapping" }));
    const row = await rowIn(await screen.findByRole("grid", { name: "OS Login mappings" }), "lena_b_corp_example");
    unavailable(within(row).getByRole("button", { name: "Edit mapping lena_b_corp_example" }));
    unavailable(within(row).getByRole("button", { name: "Delete mapping lena_b_corp_example" }));
    expect(screen.getAllByText(/Live updates paused/)).toHaveLength(1);
  }, 60_000);

  it("turns adding, changing and deleting a severity rule off, and keeps the rules on screen", async () => {
    await bringDown("/admin/severity");
    unavailable(await screen.findByRole("button", { name: "Add rule" }));
    const row = await rowIn(await screen.findByRole("grid", { name: "Severity rules" }), "Security files are High");
    unavailable(within(row).getByRole("button", { name: "Edit rule security-high" }));
    unavailable(within(row).getByRole("button", { name: "Delete rule security-high" }));
  }, 60_000);

  it("turns the dry run and saving off, and keeps the policy and its saved dry run on screen", async () => {
    await bringDown("/admin/retention");
    unavailable(await screen.findByRole("button", { name: "Run dry run" }));
    unavailable(screen.getByRole("button", { name: "Save policy" }));
    expect(screen.getByText("The saved policy: 180 days, last 50 versions")).toBeInTheDocument();
  }, 60_000);
});
