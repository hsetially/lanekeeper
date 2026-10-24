import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { PrJobState, type DomainEvent, type PrJob } from "@/api/generated/model";
import { json, problem, renderPrs, routes } from "@/features/prs/test-utils";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";
import { PrJobProgress } from "./PrJobProgress";

// The connected view of a job: `getPrJob`, kept fresh by `pr_job_progress` (src/live/invalidation.ts: it names `['pr-job', id]`,
// and the lists of links when the job ends). It starts from the job the 202 answered and moves only when the server says so.


const job = (state: PrJob["state"], over: Partial<PrJob> = {}): PrJob => ({
  id: 7,
  swimlane: "sitb",
  state,
  paths: ["tx-infinity-api/tx-infinity-core-sit7.yml"],
  pr_url: null,
  error: null,
  created_at: 1_760_000_000_000,
  updated_at: 1_760_000_060_000,
  ...over,
});
const URL419 = "https://github.com/csp-tenant/csp-tenant-data/pull/419";
const active = () => screen.getAllByRole("listitem").filter((li) => li.getAttribute("data-status") === "active").map((li) => li.textContent);

describe("PrJobProgress", () => {
  it("shows the job it was given at once, and the server's state as soon as it is read", async () => {
    let answer: (r: Response) => void = () => undefined;
    const held = new Promise<Response>((resolve) => {
      answer = resolve;
    });
    renderPrs(<PrJobProgress jobId={7} initial={job(PrJobState.pending)} />, { api: () => held });
    expect(await screen.findByText("Waiting to start.")).toBeInTheDocument();
    await act(async () => {
      answer(json(job(PrJobState.committing)));
      await held;
    });
    expect(await screen.findByText("Creating your pull request.")).toBeInTheDocument();
    expect(active()).toEqual(["Committing"]);
  });

  it("says the pull request was created when the server says the job is completed, and not before", async () => {
    renderPrs(<PrJobProgress jobId={7} initial={job(PrJobState.opening)} effect="Base changes reach every swimlane on its next sync." title="Promote tx-infinity-core from sit7 to base" />, {
      api: routes({ "GET /api/v1/pr-jobs/7": () => job(PrJobState.completed, { pr_url: URL419 }) }),
    });
    expect(await screen.findByText("PR #419 created.")).toBeInTheDocument();
    expect(screen.getByText("Base changes reach every swimlane on its next sync.")).toBeInTheDocument();
    expect(screen.getByText("Promote tx-infinity-core from sit7 to base")).toBeInTheDocument();
  });

  it("starts from nothing but the id (a job found again from a link): a loading state, then the job", async () => {
    let answer: (r: Response) => void = () => undefined;
    const held = new Promise<Response>((resolve) => {
      answer = resolve;
    });
    renderPrs(<PrJobProgress jobId={7} />, { api: () => held });
    expect(await screen.findByRole("group", { name: "Loading the pull request job" })).toBeInTheDocument();
    await act(async () => {
      answer(json(job(PrJobState.preparing)));
      await held;
    });
    expect(await screen.findByText("Creating your pull request.")).toBeInTheDocument();
  });

  it("keeps the job it was given and says it could not refresh, when the first read fails; Retry reads it again", async () => {
    let failing = true;
    const { calls } = renderPrs(<PrJobProgress jobId={7} initial={job(PrJobState.committing)} />, {
      api: (c) => (c.path === "/api/v1/pr-jobs/7" ? (failing ? problem(500, "internal") : json(job(PrJobState.opening))) : problem(404, "not_found")),
    });
    expect(await screen.findByText(/Couldn't refresh this job/, {}, { timeout: 8000 })).toBeInTheDocument();
    expect(active()).toEqual(["Committing"]);
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    failing = false;
    const before = calls.length;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(active()).toEqual(["Opening pull request"]));
    expect(calls.length).toBeGreaterThan(before);
    expect(screen.queryByText(/Couldn't refresh this job/)).toBeNull();
  });

  it("says a job it cannot find was not found, with the request ID, when it has nothing to show", async () => {
    renderPrs(<PrJobProgress jobId={7} />, { api: () => problem(404, "not_found") });
    expect(await screen.findByText("Couldn't load the pull request job", {}, { timeout: 8000 })).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("PrJobProgress, live", () => {
  let hub: FakeHub;
  let world: PrJob;
  let nextId: number;
  beforeEach(() => {
    hub = new FakeHub();
    world = job(PrJobState.committing);
    nextId = 41;
  });

  const send = async (event: DomainEvent): Promise<void> => {
    await act(async () => {
      hub.last.push(frame(nextId++, event));
      await Promise.resolve();
    });
  };

  async function open() {
    const app = renderPrs(<PrJobProgress jobId={7} initial={world} />, {
      api: routes({ "GET /api/v1/pr-jobs/7": () => world, "GET /api/v1/prs": () => ({ items: [], next: null }) }),
      live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } },
    });
    await screen.findByText("Creating your pull request.");
    await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 4000 });
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await Promise.resolve();
    });
    // The first frame of a stream with nothing to resume from asks for everything once.
    await new Promise((resolve) => setTimeout(resolve, 200));
    return { ...app, jobCalls: () => app.calls.filter((c) => c.path === "/api/v1/pr-jobs/7").length };
  }

  it("moves a step when pr_job_progress names the job, by asking the server and not by believing the event", async () => {
    const { jobCalls } = await open();
    const before = jobCalls();
    world = job(PrJobState.opening);
    await send({ type: "pr_job_progress", job_id: 7, state: "opening" });
    await waitFor(() => expect(active()).toEqual(["Opening pull request"]), { timeout: 4000 });
    expect(jobCalls()).toBe(before + 1);
  });

  it("does not say completed because an event said so: the server's answer decides", async () => {
    await open();
    // The event claims completion; the server (the source of truth) still says opening.
    world = job(PrJobState.opening);
    await send({ type: "pr_job_progress", job_id: 7, state: "completed" });
    await waitFor(() => expect(active()).toEqual(["Opening pull request"]), { timeout: 4000 });
    expect(screen.queryByText(/created/i)).toBeNull();
  });

  it("finishes with the pull request when the job completes", async () => {
    const { calls } = await open();
    world = job(PrJobState.completed, { pr_url: URL419 });
    await send({ type: "pr_job_progress", job_id: 7, state: "completed" });
    expect(await screen.findByText("PR #419 created.", {}, { timeout: 4000 })).toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/v1/pr-jobs/7")).toBe(true);
  });

  it("leaves the job alone when the event is for another job", async () => {
    const { jobCalls } = await open();
    const before = jobCalls();
    await send({ type: "pr_job_progress", job_id: 8, state: "opening" });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(jobCalls()).toBe(before);
  });
});
