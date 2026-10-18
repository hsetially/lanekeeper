import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent } from "@/api/generated/model";
import { IDS, NOW, samRequest } from "@/mocks/design/admin/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/admin/world";
import { renderApp } from "@/test/app";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// `access_request_created` (the live table in the plan; Admin only, the hub filters it) must change two things on the Admin screen
// without a reload: the count beside "Users and access requests" in the area's navigation, and the list of pending requests. The live
// layer is the real one (parser, client, plan, batcher) on a fake hub: the world gains a request, the event is pushed, and the
// screen has to show it. An event that names something else must not ask for the requests again.

const FIRST = { timeout: 8000 };
let world: World;
let hub: FakeHub;
let nextId: number;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
  hub = new FakeHub();
  nextId = 61;
  world = newWorld();
  world.requests = [];
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "cookie");
});

const isRequestList = (path: string): boolean => new URL(path, "http://hub.test").pathname === "/api/v1/admin/access-requests";

async function openLive() {
  const app = renderApp("/admin/users", worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
  await screen.findByRole("heading", { level: 1, name: "Admin" }, FIRST);
  await screen.findByText("No pending requests", {}, FIRST);
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 4000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  // A stream with nothing to resume from reads what is on screen once.
  await waitFor(() => expect(app.calls.filter((c) => isRequestList(c.path)).length).toBeGreaterThanOrEqual(2));
  await new Promise((resolve) => setTimeout(resolve, 150));
  return app;
}
const send = async (event: DomainEvent): Promise<void> => {
  await act(async () => {
    hub.last.push(frame(nextId++, event));
    await Promise.resolve();
  });
};

describe("access_request_created", () => {
  it("adds the request to the list and the count to the navigation, without a reload", async () => {
    const app = await openLive();
    const nav = screen.getByRole("navigation", { name: "Admin sections" });
    expect(within(nav).queryByLabelText(/waiting/)).toBeNull();
    world.requests = [samRequest()];
    await send({ type: "access_request_created", user: IDS.sam });
    expect(await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" }, { timeout: 3000 })).toBeInTheDocument();
    expect(await within(nav).findByLabelText("1 request waiting")).toBeInTheDocument();
    expect(screen.getByText("1 waiting")).toBeInTheDocument();
    expect(app.calls.filter((c) => isRequestList(c.path) && c.path.includes("status=pending")).length).toBeGreaterThanOrEqual(3);
  });

  it("counts every waiting request in the navigation, in the plural", async () => {
    await openLive();
    const nav = screen.getByRole("navigation", { name: "Admin sections" });
    world.requests = [samRequest(), samRequest({ id: 42, user: IDS.tomas, email: "tomas.reyes@corp.example", requested_role: "viewer", reason: null })];
    await send({ type: "access_request_created", user: IDS.tomas });
    expect(await within(nav).findByLabelText("2 requests waiting", {}, { timeout: 3000 })).toHaveTextContent("2");
    expect(screen.getByText("2 waiting")).toBeInTheDocument();
  });

  it("asks for the requests once for a burst of events, not once for each", async () => {
    const app = await openLive();
    const before = app.calls.filter((c) => isRequestList(c.path)).length;
    world.requests = [samRequest()];
    await send({ type: "access_request_created", user: IDS.sam });
    await send({ type: "access_request_created", user: IDS.sam });
    await send({ type: "access_request_created", user: IDS.sam });
    await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" }, { timeout: 3000 });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(app.calls.filter((c) => isRequestList(c.path)).length - before).toBe(1);
  });

  it("leaves the requests alone for an event that is about something else", async () => {
    const app = await openLive();
    const before = app.calls.filter((c) => isRequestList(c.path)).length;
    await send({ type: "findings_changed", swimlane: "sitb" });
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(app.calls.filter((c) => isRequestList(c.path)).length).toBe(before);
  });
});
