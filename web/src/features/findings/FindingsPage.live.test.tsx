import { act, configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent, Finding } from "@/api/generated/model";
import { DESIGN_FINDINGS, finding, laneCounts } from "@/mocks/design/findings/fixtures";
import { FakeHub, frame } from "@/test/fake-hub";
import { problem, renderApp } from "@/test/app";
import { FIRST_FRAME } from "@/test/sse-golden";
import { cellsOf, tableRows } from "@/mocks/design/findings/dom";
import { isFindings, newWorld, worldApi, type World } from "@/mocks/design/findings/world";

// The Findings screen is live. The live layer is the real one (parser, client, plan, batcher) on a fake hub: the world is changed
// between two answers, the event is pushed, and the screen has to show the new answer, the way the stable-view rule says (a row
// that changes in place changes in place; a row that comes or goes is counted and shown when the person asks). The sidebar's
// Findings count is the sum of the swimlane list's counts, so the same event moves it.

configure({ asyncUtilTimeout: 15_000 });
vi.setConfig({ testTimeout: 60_000 });

let world: World;
let hub: FakeHub;
let nextId: number;
beforeEach(() => {
  hub = new FakeHub();
  nextId = 41;
  world = newWorld(DESIGN_FINDINGS, "admin");
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

/** The screen with the stream open and past its first refetch (a stream with nothing to resume from reads everything once). */
async function openLive(url = "/findings") {
  const app = renderApp(url, worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
  await waitFor(() => expect(tableRows()).not.toHaveLength(0), { timeout: 15_000 });
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 8_000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  await waitFor(() => expect(app.calls.filter((c) => isFindings(c.path)).length).toBeGreaterThanOrEqual(2));
  await new Promise((resolve) => setTimeout(resolve, 150));
  return { app, findingCalls: () => app.calls.filter((c) => isFindings(c.path)).length };
}

const send = async (event: DomainEvent): Promise<void> => {
  await act(async () => {
    hub.last.push(frame(nextId++, event));
    await Promise.resolve();
  });
};
const findingsChanged = (swimlane = "sitb"): DomainEvent => ({ type: "findings_changed", swimlane });
const sidebarCount = (): string | null => within(screen.getByRole("complementary", { name: "Primary" })).getByRole("link", { name: /^Findings/ }).textContent;
const messages = (): string[] => tableRows().map((row) => cellsOf(row)[4] ?? "");

/** A finding of sitb that is new. */
const arrival = (): Finding => finding(12, { swimlane: "sitb", kind: "unresolved_placeholder", severity: "medium", path: "tx-infinity-api/new-sit7.yml", message: "A placeholder with no value" });

describe("a findings_changed event", () => {
  it("refetches the list and moves the sidebar's count, and holds a row that arrives until the person asks for it", async () => {
    const { findingCalls } = await openLive();
    expect(sidebarCount()).toBe("Findings11 open");
    const before = findingCalls();
    world.findings = [...world.findings, arrival()];
    world.lanes = laneCounts(world.findings);
    await send(findingsChanged());

    await waitFor(() => expect(findingCalls()).toBeGreaterThan(before));
    // The sidebar counts from the swimlane list, which the same event refetched.
    await waitFor(() => expect(sidebarCount()).toBe("Findings12 open"));
    // The list on screen is held still while it is read: the new row is counted, not inserted.
    expect(await screen.findByText("1 update")).toBeInTheDocument();
    expect(messages()).not.toContain("A placeholder with no value");
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    await waitFor(() => expect(messages()).toContain("A placeholder with no value"));
    expect(screen.queryByText("1 update")).toBeNull();
    expect(tableRows()).toHaveLength(12);
  });

  it("changes a row in place, with no count, when the rows are the same and only what one says changed", async () => {
    await openLive();
    world.findings = world.findings.map((f) => (f.id === 11 ? { ...f, message: "Duplicate keys, now at four lines", severity: "critical" as const } : f));
    await send(findingsChanged("presitb"));
    await waitFor(() => expect(messages()).toContain("Duplicate keys, now at four lines"));
    expect(screen.queryByText(/updates?$/)).toBeNull();
    // The server's new severity is shown as the server gave it.
    const row = tableRows().find((r) => (cellsOf(r)[4] ?? "").startsWith("Duplicate keys, now"));
    expect(row && cellsOf(row)[0]).toBe("Critical");
  });

  it("keeps an open finding open while its row is held, and says it is no longer open once the list is shown", async () => {
    await openLive();
    await userEvent.click(tableRows().find((r) => (cellsOf(r)[4] ?? "").startsWith("Duplicate keys")) as HTMLElement);
    await screen.findByRole("dialog", { name: "Duplicate keys" });
    world.findings = world.findings.filter((f) => f.id !== 11);
    world.lanes = laneCounts(world.findings);
    await send(findingsChanged("presitb"));
    expect(await screen.findByText("1 update")).toBeInTheDocument();
    // Still the finding that was opened, as it was.
    expect(screen.getByRole("dialog", { name: "Duplicate keys" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    const gone = await screen.findByRole("dialog", { name: "This finding is no longer open" });
    expect(within(gone).getByText(/The check no longer reports it/)).toBeInTheDocument();
    await userEvent.click(within(gone).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(tableRows()).toHaveLength(10);
  });

  it("refetches a filtered list too, since one event covers every filter", async () => {
    const { findingCalls } = await openLive("/findings?lanes=sitb");
    expect(tableRows()).toHaveLength(3);
    const before = findingCalls();
    world.findings = [...world.findings, arrival()];
    world.lanes = laneCounts(world.findings);
    await send(findingsChanged());
    await waitFor(() => expect(findingCalls()).toBeGreaterThan(before));
    expect(await screen.findByText("1 update")).toBeInTheDocument();
  });

  it("keeps the findings on screen and says so when the refetch fails", async () => {
    await openLive();
    world.findingsAnswer = () => problem(500, "internal", { title: "The registry is busy" });
    await send(findingsChanged());
    expect(await screen.findByText("Couldn't refresh")).toBeInTheDocument();
    expect(screen.getByText(/Showing the last data that loaded\./)).toBeInTheDocument();
    expect(tableRows()).toHaveLength(11);
  });

  it("is the only kind of event that asks for the findings again", async () => {
    const { findingCalls } = await openLive();
    const before = findingCalls();
    await send({ type: "pickup_changed", swimlane: "sitb", services: [{ namespace: "sitb", name: "tx-infinity-core" }] });
    await send({ type: "agent_status_changed", swimlane: "sitb", status: "disconnected" });
    await new Promise((resolve) => setTimeout(resolve, 600));
    expect(findingCalls()).toBe(before);
  });
});

// State U04: the stream is down for three attempts in a row. The screen keeps what it has and says that it may be out of date;
// it has no write to turn off.
describe("when live updates are paused", () => {
  // Retries after a few milliseconds instead of seconds: the states are the same, the test is not slow.
  const fastTimers = {
    setTimeout: (run: () => void, ms: number) => globalThis.setTimeout(run, Math.min(ms, 5)),
    clearTimeout: (handle: ReturnType<typeof setTimeout>) => {
      globalThis.clearTimeout(handle);
    },
  };

  it("keeps the findings on screen, and the shell says once that live updates are paused: the list adds no second banner", async () => {
    renderApp("/findings", worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, timers: fastTimers, onlineTarget: new EventTarget() } } });
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
    expect(screen.getAllByText("Live updates paused, retrying")).toHaveLength(1);
    expect(screen.queryByText(/so this view may be out of date/)).toBeNull();
    expect(tableRows()).toHaveLength(11);
  }, 90_000);
});
