import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { DomainEvent, SwimlaneSummary } from "@/api/generated/model";
import { EVENT_KINDS } from "@/live/events";
import { prefs } from "@/lib/prefs";
import { lane } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { FakeHub, frame } from "@/test/fake-hub";
import { renderApp } from "@/test/app";
import { FIRST_FRAME } from "@/test/sse-golden";

// The swimlane list is live: each event of the stream (the plan's invalidation table) either changes a row of the list as the
// table says, or leaves the list alone. The live layer is the real one (parser, client, plan, batcher) on a fake hub; the
// world is changed between two answers, the event is pushed, and the screen has to show the new answer. An event that does not
// touch swimlanes is proved by the list not being asked for again.

const HASH = "a".repeat(64);
const COMMIT = "b".repeat(40);
const SERVICES = [{ namespace: "sitb", name: "tx-infinity-core" }];
const JOB = { name: "csp-tenant-data-sit7", uid: "run-42" };
const HOUR = 3_600_000;

const base = (): SwimlaneSummary[] => [
  lane("sit", { last_scan_at: Date.now() - 3 * HOUR }),
  lane("sitb", { tenants: ["sit7"], last_scan_at: Date.now() - 2 * HOUR, drift_counts: [["nfs_ahead", 2]], finding_count: 2, base_version: { tag: "v2.3.1", distance: 4, commit: COMMIT } }),
  lane("sitc", { last_scan_at: Date.now() - 5 * HOUR }),
];

let world: World;
let hub: FakeHub;
let nextId: number;
beforeEach(() => {
  prefs.clear();
  hub = new FakeHub();
  nextId = 41;
  world = newWorld("viewer", base());
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const sitb = (over: Partial<SwimlaneSummary>): void => {
  world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, ...over } : l));
};
const isList = (path: string): boolean => new URL(path, "http://hub.test").pathname === "/api/v1/swimlanes";

/** The list on screen with the stream open and past its first refetch (a stream with nothing to resume from reads everything once). */
async function openLive() {
  const app = renderApp("/swimlanes", worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
  // The first screen of a run loads the page's chunk and the live engine's, which takes longer than the default wait.
  await screen.findByRole("link", { name: "sitb" }, { timeout: 8000 });
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 4000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  await waitFor(() => expect(app.calls.filter((c) => isList(c.path)).length).toBeGreaterThanOrEqual(2));
  await new Promise((resolve) => setTimeout(resolve, 150));
  return { app, listCalls: () => app.calls.filter((c) => isList(c.path)).length };
}

const send = async (event: DomainEvent | { type: "resync" }): Promise<void> => {
  await act(async () => {
    hub.last.push(event.type === "resync" ? "event: resync\ndata: {\"type\":\"resync\"}\n\n" : frame(nextId++, event));
    await Promise.resolve();
  });
};
const rowOf = async (name: string): Promise<HTMLElement> => {
  const link = await screen.findByRole("link", { name });
  const row = link.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${name}`);
  return row;
};
const seen = (name: string, check: (row: HTMLElement) => void) =>
  waitFor(
    async () => {
      check(await rowOf(name));
    },
    { timeout: 3000 },
  );

type Case = {
  kind: DomainEvent["type"];
  event: DomainEvent | { type: "resync" };
  /** What the server answers from now on. */
  change: () => void;
  /** What the sitb row says once the event has been taken. */
  expectRow: (row: HTMLElement) => void;
};

const SYNCING = { job: JOB, since: Date.now() };

// Every event that names a swimlane (or all of them) and so changes the list. The row is sitb.
const CHANGES: Case[] = [
  {
    kind: "file_observed",
    event: { type: "file_observed", swimlane: "sitb", path: "tx-infinity-api/core.yml", hash: HASH, source: "tool_write" },
    change: () => sitb({ last_scan_at: Date.now() }),
    expectRow: (row) => expect(within(row).getByText("just now")).toBeInTheDocument(),
  },
  {
    kind: "drift_changed",
    event: { type: "drift_changed", swimlane: "sitb", paths: ["tx-infinity-api/core.yml"] },
    change: () => sitb({ drift_counts: [["conflict", 3]] }),
    expectRow: (row) => {
      expect(within(row).getByRole("img", { name: "3 Conflict" })).toBeInTheDocument();
      expect(within(row).queryByRole("img", { name: "2 NFS ahead" })).toBeNull();
    },
  },
  {
    kind: "findings_changed",
    event: { type: "findings_changed", swimlane: "sitb" },
    change: () => sitb({ finding_count: 7 }),
    expectRow: (row) => expect(within(row).getByText("7")).toBeInTheDocument(),
  },
  {
    kind: "pickup_changed",
    event: { type: "pickup_changed", swimlane: "sitb", services: SERVICES },
    change: () => sitb({ pickup_counts: [["needs_config_server_restart", 1]] }),
    expectRow: (row) => expect(within(row).getByRole("img", { name: "1 Needs config-server restart" })).toBeInTheDocument(),
  },
  {
    kind: "pending_restart_changed",
    event: { type: "pending_restart_changed", swimlane: "sitb", services: SERVICES },
    change: () => sitb({ pickup_counts: [["needs_notify_or_restart", 3]] }),
    expectRow: (row) => expect(within(row).getByRole("img", { name: "3 Needs notify or restart" })).toBeInTheDocument(),
  },
  {
    kind: "agent_status_changed",
    event: { type: "agent_status_changed", swimlane: "sitb", status: "disconnected" },
    change: () => sitb({ agent: "disconnected" }),
    expectRow: (row) => {
      expect(within(row).getByText(/^Offline · /)).toBeInTheDocument();
      expect(within(row).getByText(/^Stale · last known 2 NFS ahead/)).toBeInTheDocument();
    },
  },
  {
    kind: "git_head_moved",
    event: { type: "git_head_moved", repo: "base", branch: "main", from: COMMIT, to: "c".repeat(40) },
    change: () => sitb({ base_version: { tag: "v2.3.1", distance: 5, commit: "c".repeat(40) } }),
    expectRow: (row) => expect(within(row).getByText("v2.3.1 + 5")).toBeInTheDocument(),
  },
  {
    kind: "sync_window_opened",
    event: { type: "sync_window_opened", swimlane: "sitb", job: JOB },
    change: () => sitb({ syncing: SYNCING }),
    expectRow: (row) => expect(within(row).getByText("Syncing")).toBeInTheDocument(),
  },
  {
    kind: "sync_window_closed",
    event: { type: "sync_window_closed", swimlane: "sitb", job: JOB },
    change: () => sitb({ syncing: null }),
    expectRow: (row) => expect(within(row).queryByText("Syncing")).toBeNull(),
  },
  {
    kind: "swimlane_changed",
    event: { type: "swimlane_changed", swimlane: "sitb" },
    change: () => sitb({ baseline: { state: "in_progress", by: null, at: null }, tier: "PRESIT", tenants: ["sit7", "sit8"] }),
    expectRow: (row) => {
      expect(within(row).getByText("Adoption in progress")).toBeInTheDocument();
      expect(within(row).getByText("PRESIT")).toBeInTheDocument();
      expect(within(row).getByText("+1")).toBeInTheDocument();
    },
  },
  {
    kind: "resync",
    event: { type: "resync" },
    change: () => sitb({ finding_count: 9, drift_counts: [["git_ahead", 4]] }),
    expectRow: (row) => {
      expect(within(row).getByText("9")).toBeInTheDocument();
      expect(within(row).getByRole("img", { name: "4 Git ahead" })).toBeInTheDocument();
    },
  },
];

// Every other event of the table names something that is not in the swimlane list, so the list is not asked for again.
const LEAVES: Array<{ kind: DomainEvent["type"]; event: DomainEvent }> = [
  { kind: "proposal_changed", event: { type: "proposal_changed", id: 12, status: "approved" } },
  { kind: "access_request_created", event: { type: "access_request_created", user: "11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa" } },
  { kind: "doc_indexed", event: { type: "doc_indexed", doc: 5 } },
  { kind: "attribution_updated", event: { type: "attribution_updated", swimlane: "sitb", path: "tx-infinity-api/core.yml", hash: HASH } },
  { kind: "pr_state_changed", event: { type: "pr_state_changed", link_id: 3, state: "merged" } },
  { kind: "pr_job_progress", event: { type: "pr_job_progress", job_id: 4, state: "completed" } },
];

describe("the events that change the list", () => {
  it.each(CHANGES)("$kind changes the row it names, as the invalidation table says", async ({ event, change, expectRow }) => {
    await openLive();
    expect(within(await rowOf("sitb")).getByText("2 h ago")).toBeInTheDocument();
    change();
    await send(event);
    await seen("sitb", expectRow);
  });

  it("refetches the list once for a burst of events, not once for each", async () => {
    const { listCalls } = await openLive();
    const before = listCalls();
    sitb({ finding_count: 5 });
    await send({ type: "findings_changed", swimlane: "sitb" });
    await send({ type: "findings_changed", swimlane: "sitb" });
    await send({ type: "drift_changed", swimlane: "sitb", paths: ["a.yml"] });
    await seen("sitb", (row) => expect(within(row).getByText("5")).toBeInTheDocument());
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(listCalls() - before).toBe(1);
  });
});

describe("the events that leave the list alone", () => {
  it.each(LEAVES)("$kind does not ask for the swimlanes again", async ({ event }) => {
    const { listCalls } = await openLive();
    const before = listCalls();
    await send(event);
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(listCalls()).toBe(before);
  });
});

describe("the table covers the stream", () => {
  it("has a test for every kind of event the client understands", () => {
    const covered = new Set<string>([...CHANGES.map((c) => c.kind), ...LEAVES.map((c) => c.kind)]);
    expect([...covered].sort()).toEqual([...EVENT_KINDS].sort());
  });
});

describe("what a live update does to what the person is doing", () => {
  it("marks the row that changed in place, for a while, and moves nothing", async () => {
    await openLive();
    const before = (await screen.findAllByRole("link")).map((a) => a.getAttribute("href"));
    sitb({ last_scan_at: Date.now() });
    await send({ type: "file_observed", swimlane: "sitb", path: "a.yml", hash: HASH, source: "tool_write" });
    await seen("sitb", (row) => expect(within(row).getByText("just now")).toHaveAttribute("data-changed", "true"));
    // The whole row is tinted (the design's highlight), not only the time; the other rows are not.
    await seen("sitb", (row) => expect(row.firstElementChild).toHaveClass("bg-info-bg"));
    await seen("sitc", (row) => expect(row.firstElementChild).not.toHaveClass("bg-info-bg"));
    const after = (await screen.findAllByRole("link")).map((a) => a.getAttribute("href"));
    expect(after).toEqual(before);
  });

  it("keeps the text of the filter and the rows that are selected", async () => {
    await openLive();
    await userEvent.click(await screen.findByRole("checkbox", { name: "Select sitc" }));
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter swimlanes" }), "sit");
    sitb({ finding_count: 8 });
    await send({ type: "findings_changed", swimlane: "sitb" });
    await seen("sitb", (row) => expect(within(row).getByText("8")).toBeInTheDocument());
    expect(screen.getByRole("searchbox", { name: "Filter swimlanes" })).toHaveValue("sit");
    expect(screen.getByRole("checkbox", { name: "Select sitc" })).toBeChecked();
  });

  it("holds a swimlane that arrives back behind a count, and shows it when asked", async () => {
    await openLive();
    world.lanes = [...world.lanes, lane("sitd", { last_scan_at: Date.now() })];
    await send({ type: "swimlane_changed", swimlane: "sitd" });
    expect(await screen.findByText("1 update")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "sitd" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    expect(await screen.findByRole("link", { name: "sitd" })).toBeInTheDocument();
    expect(screen.queryByText("1 update")).toBeNull();
  });

  it("holds a swimlane that goes away the same way", async () => {
    await openLive();
    world.lanes = world.lanes.filter((l) => l.id !== "sitc");
    await send({ type: "swimlane_changed", swimlane: "sitc" });
    expect(await screen.findByText("1 update")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "sitc" })).toBeInTheDocument();
  });
});
