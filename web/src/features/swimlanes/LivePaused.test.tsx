import { act, configure, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { prefs } from "@/lib/prefs";
import { lane, service } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { FakeHub } from "@/test/fake-hub";
import { renderApp } from "@/test/app";
import { FIRST_FRAME } from "@/test/sse-golden";

// "Live updates paused" (States U01 and U02): the stream is down for three attempts in a row. The screen keeps what it has, says
// that it may be out of date, and every write is off with the reason; the stream coming back turns them on again.

vi.mock("@/components/ui/resizable", async () => import("@/mocks/design/swimlanes/plain-resizable"));
configure({ asyncUtilTimeout: 15_000 });

// The two screens are lazy chunks of hundreds of modules, and the first import of them is slow when the machine is busy (the full run
// with other work going on took more than a test's wait). It is not what these tests wait for: it is done here, once, with a timeout
// of its own, so that a wait in a test is a wait for the stream and the screen.
beforeAll(async () => {
  await Promise.all([import("@/features/swimlanes/SwimlanesPage"), import("@/features/swimlanes/ServicesPage")]);
}, 180_000);

let world: World;
let hub: FakeHub;
beforeEach(() => {
  prefs.clear();
  hub = new FakeHub();
  world = newWorld("operator", [lane("sit"), lane("sitb", { last_scan_at: Date.now() - 5_000 })]);
  world.services["sitb"] = [service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: Date.now() - 60_000 })];
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

// Retries after a few milliseconds instead of seconds: the states are the same, the test is not slow.
const fastTimers = {
  setTimeout: (run: () => void, ms: number) => globalThis.setTimeout(run, Math.min(ms, 5)),
  clearTimeout: (handle: ReturnType<typeof setTimeout>) => {
    globalThis.clearTimeout(handle);
  },
};

async function bringDown(url: string) {
  const app = renderApp(url, worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, timers: fastTimers, onlineTarget: new EventTarget() } } });
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 25_000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  // The hub stays down: every attempt after the open stream breaks is refused, so the screen stays in the paused state.
  for (let i = 0; i < 400; i++) hub.respond(503);
  await act(async () => {
    hub.last.fail();
    await Promise.resolve();
  });
  await screen.findByText("Live updates paused, retrying", {}, { timeout: 25_000 });
  return app;
}

describe("when live updates are paused", () => {
  it("keeps the swimlane list on screen, and says once, in the shell, that live updates are paused: the list adds no second banner", async () => {
    await bringDown("/swimlanes");
    expect(await screen.findByRole("link", { name: "sitb" })).toBeInTheDocument();
    expect(screen.getAllByText("Live updates paused, retrying")).toHaveLength(1);
    expect(screen.queryByText("Live updates paused")).toBeNull();
    expect(screen.queryByText(/so this view may be out of date/)).toBeNull();
  }, 60_000);

  it("turns Restart off on the Services tab and says why: unavailable offline", async () => {
    await bringDown("/swimlanes/sitb/services");
    const button = await screen.findByRole("button", { name: "Restart tx-infinity-core" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Unavailable offline");
    // What was on screen stays: the service and what it waits on.
    expect(within(screen.getByRole("table", { name: "Services" })).getByText("Needs notify or restart")).toBeInTheDocument();
  }, 60_000);
});
