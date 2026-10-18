import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent } from "@/api/generated/model";
import { ARRIVAL, DANA, NOW, PENDING } from "@/mocks/design/approvals/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/approvals/world";
import { problem, renderApp } from "@/test/app";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// The queue is live: `proposal_changed` names the lists and the proposal (src/live/invalidation.ts), and the screen has to
// show what the server answers next. The live layer is the real one (parser, client, plan, batcher) on a fake hub; the world is
// changed between two answers, the event is pushed, and the screen has to say so. The sidebar's badge reads the same request.

let world: World;
let hub: FakeHub;
let nextId: number;
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  hub = new FakeHub();
  nextId = 41;
  world = newWorld("admin", PENDING.slice(0, 2));
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "cookie");
});

const isPendingList = (path: string): boolean => {
  const url = new URL(path, "http://hub.test");
  return url.pathname === "/api/v1/proposals" && url.search.includes("status=pending");
};

/** The queue on screen with the stream open and past its first refetch (a stream with nothing to resume from reads everything once). */
async function openLive(url = "/approvals") {
  const app = renderApp(url, worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
  await screen.findByRole("heading", { level: 1, name: "Approvals" }, { timeout: 8000 });
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 4000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  await waitFor(() => expect(app.calls.filter((c) => isPendingList(c.path)).length).toBeGreaterThanOrEqual(2));
  await new Promise((resolve) => setTimeout(resolve, 150));
  return { app, listCalls: () => app.calls.filter((c) => isPendingList(c.path)).length };
}

const send = async (event: DomainEvent): Promise<void> => {
  await act(async () => {
    hub.last.push(frame(nextId++, event));
    await Promise.resolve();
  });
};

describe("proposal_changed", () => {
  it("asks for the pending list again, once for the queue and the sidebar badge, and holds the queue still until asked", async () => {
    const { listCalls } = await openLive();
    await screen.findByRole("grid", { name: "Proposals" });
    expect(screen.getByRole("link", { name: /^Approvals/ })).toHaveTextContent("2 pending");
    const before = listCalls();

    world.proposals = [ARRIVAL, ...world.proposals];
    await send({ type: "proposal_changed", id: ARRIVAL.id, status: "pending" });

    // The counts follow the data at once; the rows wait.
    await waitFor(() => expect(screen.getByRole("link", { name: /^Approvals/ })).toHaveTextContent("3 pending"), { timeout: 3000 });
    expect(listCalls()).toBe(before + 1);
    expect(await screen.findByText("3 waiting for an Admin · 0 stale · you are signed in as Priya Shah")).toBeInTheDocument();
    const chip = await screen.findByText("1 update");
    expect(chip).toBeInTheDocument();
    expect(screen.queryByText("kafka-topics-sit11.yml")).toBeNull();

    // Show it: the new proposal is on top, tinted, with a New chip.
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    const row = (await screen.findByText("kafka-topics-sit11.yml")).closest<HTMLElement>('[role="row"]');
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).getByText("New")).toBeInTheDocument();
    expect((row as HTMLElement).firstElementChild).toHaveClass("bg-info-bg");
    expect(screen.queryByText("1 update")).toBeNull();
  });

  it("updates a row in place when only what it says changed", async () => {
    const { listCalls } = await openLive();
    await screen.findByRole("grid", { name: "Proposals" });
    const before = listCalls();
    world.proposals = world.proposals.map((p) => (p.id === 2046 ? { ...p, expires_at: NOW + 2 * 3_600_000 } : p));
    await send({ type: "proposal_changed", id: 2046, status: "pending" });
    await waitFor(() => expect(listCalls()).toBe(before + 1));
    const row = (await screen.findByText("tx-infinity-core-sit7.yml")).closest<HTMLElement>('[role="row"]');
    await waitFor(() => expect(within(row as HTMLElement).getByText("2 h")).toHaveClass("font-semibold"));
    expect(screen.queryByText(/^\d+ updates?$/)).toBeNull();
  });

  it("takes a proposal off the page when somebody else decides it, and says so in the open drawer", async () => {
    await openLive("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByRole("button", { name: "Approve" })).toBeInTheDocument();

    world.proposals = world.proposals.map((p) => (p.id === 2046 ? { ...p, status: "applied", decided_by: DANA, decided_at: NOW - 8_000 } : p));
    await send({ type: "proposal_changed", id: 2046, status: "applied" });

    expect(await within(drawer).findByText("Approved by Dana Okoye, just now", {}, { timeout: 3000 })).toBeInTheDocument();
    expect(within(drawer).getByText(/Someone else decided while you had it open\./)).toBeInTheDocument();
    expect(within(drawer).queryByRole("button", { name: "Approve" })).toBeNull();
    expect(within(drawer).queryByRole("button", { name: "Reject" })).toBeNull();
    expect(within(drawer).getByText("Decision recorded in the audit log.")).toBeInTheDocument();
    // The queue behind it stays on the tab it opened on; the row has left Pending (held, counted).
    expect(screen.getByRole("tab", { name: /^Pending/ })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByText("1 update")).toBeInTheDocument();
  });

  it("asks the detail again by its id, and the lists, and nothing else of the approvals", async () => {
    const { app } = await openLive("/approvals/2046");
    await screen.findByRole("dialog", { name: "Proposal detail" });
    const detail = () => app.calls.filter((c) => c.path === "/api/v1/proposals/2046").length;
    const before = detail();
    await send({ type: "proposal_changed", id: 2046, status: "stale" });
    await waitFor(() => expect(detail()).toBe(before + 1), { timeout: 3000 });
    // Another proposal's detail is not asked for.
    expect(app.calls.some((c) => /^\/api\/v1\/proposals\/\d+$/.test(c.path) && c.path !== "/api/v1/proposals/2046")).toBe(false);
  });
});

describe("a refresh that fails", () => {
  it("keeps the queue on screen and says that it could not refresh, with a way to try again", async () => {
    await openLive();
    await screen.findByRole("grid", { name: "Proposals" });
    world.listProblem = problem(500, "internal");
    await send({ type: "proposal_changed", id: 2046, status: "pending" });
    expect(await screen.findByText("Couldn't refresh", {}, { timeout: 3000 })).toBeInTheDocument();
    // What was there is still there.
    expect(screen.getByRole("grid", { name: "Proposals" })).toBeInTheDocument();
    expect(screen.getByText("tx-infinity-core-sit7.yml")).toBeInTheDocument();
    // Trying again reads the list as the hub answers now.
    world.listProblem = null;
    await userEvent.click(screen.getAllByRole("button", { name: "Refresh" })[0] as HTMLElement);
    await waitFor(() => expect(screen.queryByText("Couldn't refresh")).toBeNull(), { timeout: 3000 });
  });
});

