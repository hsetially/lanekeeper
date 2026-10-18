import { act, configure, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { installAdminClock } from "@/mocks/design/admin/harness";
import { newWorld, worldApi, type World } from "@/mocks/design/admin/world";
import { renderApp } from "@/test/app";
import { FakeHub } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// "Live updates paused" (States U rows) on the Admin screens: the stream is down for three attempts in a row. What is on screen stays
// and says it may be out of date, and every write is off with the reason (unavailable offline) instead of failing when it is tried.

installAdminClock();
configure({ asyncUtilTimeout: 15_000 });

let world: World;
let hub: FakeHub;
beforeEach(() => {
  hub = new FakeHub();
  world = newWorld();
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
  const app = renderApp(url, worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, timers: fastTimers, onlineTarget: new EventTarget() } } });
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

describe("when live updates are paused", () => {
  it("keeps the pending requests on screen, says once that updates are paused, and turns approving and rejecting off with the reason", async () => {
    await bringDown("/admin/users");
    const card = await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" });
    // The shell says once that updates are paused; the lists on this screen do not repeat it.
    expect(screen.getAllByText(/Live updates paused/)).toHaveLength(1);
    expect(screen.queryByText("Live updates are paused, so this view may be out of date.", { exact: false })).toBeNull();
    for (const name of ["Approve request from sam.okafor@corp.example", "Reject request from sam.okafor@corp.example"]) {
      const button = within(card).getByRole("button", { name });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).toHaveAccessibleDescription("Unavailable offline");
    }
  }, 60_000);

  it("turns the controls of the people off: role, approval, disabling", async () => {
    await bringDown("/admin/users");
    const grid = await screen.findByRole("grid", { name: "Users" });
    expect(await within(grid).findByLabelText("Role for Lena Brandt")).toBeDisabled();
    expect(within(grid).getByLabelText("Role for Lena Brandt")).toHaveAttribute("title", "Unavailable offline");
    expect(within(grid).getByRole("switch", { name: "Requires approval for Lena Brandt" })).toBeDisabled();
    expect(within(grid).getByRole("button", { name: "Disable Lena Brandt" })).toHaveAttribute("aria-disabled", "true");
  }, 60_000);

  it("turns adding a rule off, and keeps the rules on screen", async () => {
    await bringDown("/admin/paths");
    expect(await screen.findByRole("grid", { name: "Path mappings" })).toBeInTheDocument();
    const add = await screen.findByRole("button", { name: "Add rule" });
    expect(add).toHaveAttribute("aria-disabled", "true");
    expect(add).toHaveAccessibleDescription("Unavailable offline");
    const row = await within(screen.getByRole("grid", { name: "Path mappings" })).findByText("default");
    const edit = within(row.closest<HTMLElement>('[role="row"]') ?? document.body).getByRole("button", { name: "Edit rule default" });
    expect(edit).toHaveAttribute("aria-disabled", "true");
  }, 60_000);

  it("turns the auto-notify switch, the tenants and the join token off", async () => {
    await bringDown("/admin/swimlanes");
    const manage = await screen.findByRole("button", { name: "Manage sitb" });
    await act(async () => {
      manage.click();
      await Promise.resolve();
    });
    const panel = await screen.findByRole("region", { name: "Settings for sitb" });
    expect(await within(panel).findByRole("switch", { name: "Auto-notify for sitb" })).toBeDisabled();
    expect(within(panel).getByRole("button", { name: "Create join token" })).toHaveAttribute("aria-disabled", "true");
    expect(await within(panel).findByRole("button", { name: "Edit tenants of sitb" })).toHaveAttribute("aria-disabled", "true");
  }, 60_000);
});
