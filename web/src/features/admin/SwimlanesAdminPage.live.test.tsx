import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent } from "@/api/generated/model";
import { NOW } from "@/mocks/design/admin/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/admin/world";
import { renderApp } from "@/test/app";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// The agents list is the live one of the Admin area (`agent_status_changed`), and the settings of the swimlane being managed follow
// `swimlane_changed` (another Admin changed its tenants or its auto-notify switch). The live layer is the real one on a fake hub.

const FIRST = { timeout: 8000 };
let world: World;
let hub: FakeHub;
let nextId: number;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
  hub = new FakeHub();
  nextId = 81;
  world = newWorld();
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "cookie");
});

const isAgents = (path: string): boolean => new URL(path, "http://hub.test").pathname === "/api/v1/admin/agents";

async function openLive() {
  const app = renderApp("/admin/swimlanes", worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
  await screen.findByRole("heading", { level: 1, name: "Admin" }, FIRST);
  await screen.findByRole("grid", { name: "Agents" }, FIRST);
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 4000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  await waitFor(() => expect(app.calls.filter((c) => isAgents(c.path)).length).toBeGreaterThanOrEqual(2));
  await new Promise((resolve) => setTimeout(resolve, 150));
  return app;
}
const send = async (event: DomainEvent): Promise<void> => {
  await act(async () => {
    hub.last.push(frame(nextId++, event));
    await Promise.resolve();
  });
};
async function agentRow(swimlane: string): Promise<HTMLElement> {
  const grid = await screen.findByRole("grid", { name: "Agents" });
  const cell = await within(grid).findByText(swimlane);
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no agent row for ${swimlane}`);
  return row;
}

describe("agent_status_changed", () => {
  it("changes the status of the agent in the list, without a reload", async () => {
    await openLive();
    expect(within(await agentRow("sitb")).getByText("Online")).toBeInTheDocument();
    world.agents = world.agents.map((a) => (a.swimlane === "sitb" ? { ...a, status: "disconnected" as const, connected_replica: null } : a));
    await send({ type: "agent_status_changed", swimlane: "sitb", status: "disconnected" });
    await waitFor(async () => {
      expect(within(await agentRow("sitb")).getByText("Offline")).toBeInTheDocument();
    });
    expect(screen.getByText("6 registered · 2 offline · 1 never seen")).toBeInTheDocument();
  });
});

describe("swimlane_changed", () => {
  it("reads the tenants and the auto-notify switch of the swimlane being managed again", async () => {
    const user = userEvent.setup();
    await openLive();
    await user.click(await screen.findByRole("button", { name: "Manage sitb" }));
    const panel = await screen.findByRole("region", { name: "Settings for sitb" });
    expect(await within(panel).findByText("sit7")).toBeInTheDocument();
    expect(await within(panel).findByRole("switch", { name: "Auto-notify for sitb" })).toBeChecked();
    world.tenants["sitb"] = ["sit7", "sit8"];
    world.autoNotify["sitb"] = false;
    await send({ type: "swimlane_changed", swimlane: "sitb" });
    expect(await within(panel).findByText("sit8", {}, { timeout: 3000 })).toBeInTheDocument();
    await waitFor(() => expect(within(panel).getByRole("switch", { name: "Auto-notify for sitb" })).not.toBeChecked());
  });

  it("leaves the settings alone when another swimlane changed", async () => {
    const user = userEvent.setup();
    const app = await openLive();
    await user.click(await screen.findByRole("button", { name: "Manage sitb" }));
    const panel = await screen.findByRole("region", { name: "Settings for sitb" });
    await within(panel).findByText("sit7");
    const settingCalls = () => app.calls.filter((c) => /\/swimlanes\/sitb\/(tenants|auto-notify)/.test(c.path)).length;
    const before = settingCalls();
    await send({ type: "swimlane_changed", swimlane: "sitc" });
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(settingCalls()).toBe(before);
  });
});
