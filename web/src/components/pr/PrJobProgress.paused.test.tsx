import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PrJobState, type PrJob } from "@/api/generated/model";
import { renderPrs, routes } from "@/features/prs/test-utils";
import { PrJobProgress } from "./PrJobProgress";

// With the live stream down no `pr_job_progress` arrives and the app does not poll, so a job on screen may be behind the server.
// The view says so and reads the job on request. A job that has ended cannot be behind, so it says nothing.

type View = { status: string; failures: number; retryAt: number | null; dataAsOf: number | null; stopReason: string | null };
const stream = vi.hoisted((): { view: View } => ({ view: { status: "live", failures: 0, retryAt: null, dataAsOf: null, stopReason: null } }));
vi.mock("@/live/LiveProvider", async (original) => ({ ...(await original<typeof import("@/live/LiveProvider")>()), useLiveStatus: () => stream.view }));

beforeEach(() => {
  stream.view = { status: "live", failures: 0, retryAt: null, dataAsOf: null, stopReason: null };
});

const job = (state: PrJob["state"], over: Partial<PrJob> = {}): PrJob => ({
  id: 7,
  swimlane: "sitb",
  state,
  paths: ["application.properties"],
  pr_url: null,
  error: null,
  created_at: 1_760_000_000_000,
  updated_at: 1_760_000_060_000,
  ...over,
});

describe("PrJobProgress while live updates are paused", () => {
  it.each(["offline", "stopped"])("says the view may be behind when the stream is %s, and reads the job on request", async (status) => {
    stream.view = { ...stream.view, status };
    let world = job(PrJobState.committing);
    const { calls } = renderPrs(<PrJobProgress jobId={7} initial={world} />, { api: routes({ "GET /api/v1/pr-jobs/7": () => world }) });
    expect(await screen.findByText(/Live updates are paused, so this may be behind the server/)).toBeInTheDocument();
    const before = calls.filter((c) => c.path === "/api/v1/pr-jobs/7").length;
    world = job(PrJobState.opening);
    await userEvent.click(screen.getByRole("button", { name: "Check now" }));
    await waitFor(() => expect(calls.filter((c) => c.path === "/api/v1/pr-jobs/7").length).toBeGreaterThan(before));
    await waitFor(() => expect(screen.getAllByRole("listitem").filter((li) => li.getAttribute("data-status") === "active").map((li) => li.textContent)).toEqual(["Opening pull request"]));
  });

  it("says nothing while the stream is live", async () => {
    renderPrs(<PrJobProgress jobId={7} initial={job(PrJobState.committing)} />, { api: routes({ "GET /api/v1/pr-jobs/7": () => job(PrJobState.committing) }) });
    await screen.findByText("Creating your pull request.");
    expect(screen.queryByText(/Live updates are paused/)).toBeNull();
  });

  it.each([
    [PrJobState.completed, { pr_url: "https://github.com/csp/x/pull/5" }],
    [PrJobState.failed, {}],
  ] as const)("says nothing for a %s job: it cannot change again", async (state, over) => {
    stream.view = { ...stream.view, status: "offline" };
    renderPrs(<PrJobProgress jobId={7} initial={job(state, over)} />, { api: routes({ "GET /api/v1/pr-jobs/7": () => job(state, over) }) });
    await screen.findByText(state === PrJobState.completed ? "PR #5 created." : "The pull request job failed.");
    expect(screen.queryByText(/Live updates are paused/)).toBeNull();
  });
});
