import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { PrJobState, type PrJob } from "@/api/generated/model";
import { renderPrs } from "@/features/prs/test-utils";
import { PrJobView } from "./PrJobView";

// Consequence dialogs, frames 1g to 1i: the job as the server reports it. Progress is the job's state and nothing else: a step is
// done because the state is past it, "created" is said only for a completed job, and a job that is not completed shows no pull
// request. The job is stored on the server and the page cannot keep its id (no web storage), so a person who loses the page is
// given a way to find the pull request in the list instead of a resumed job.


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

const stepsOf = () => screen.getAllByRole("listitem").map((li) => `${li.textContent ?? ""}:${li.getAttribute("data-status") ?? ""}`);

describe("PrJobView, queued", () => {
  it("has begun nothing: all four steps wait, and it says it is waiting to start", async () => {
    renderPrs(<PrJobView job={job(PrJobState.pending)} />);
    expect(await screen.findByText("Waiting to start.")).toBeInTheDocument();
    expect(stepsOf()).toEqual(["Preparing:waiting", "Committing:waiting", "Opening pull request:waiting", "Done:waiting"]);
  });
});

describe("PrJobView, running", () => {
  it.each([
    [PrJobState.preparing, ["Preparing:active", "Committing:waiting", "Opening pull request:waiting", "Done:waiting"]],
    [PrJobState.committing, ["Preparing:done", "Committing:active", "Opening pull request:waiting", "Done:waiting"]],
    [PrJobState.opening, ["Preparing:done", "Committing:done", "Opening pull request:active", "Done:waiting"]],
  ] as const)("%s is drawn as the server says", async (state, steps) => {
    renderPrs(<PrJobView job={job(state)} />);
    expect(await screen.findByText("Creating your pull request.")).toBeInTheDocument();
    expect(stepsOf()).toEqual(steps);
  });

  it("tells the person the job goes on without this page, and where to find the pull request", async () => {
    renderPrs(<PrJobView job={job(PrJobState.committing)} />);
    expect(await screen.findByText(/You can close this dialog\. The job keeps running/)).toBeInTheDocument();
    const link = screen.getByRole("link", { name: "Find my pull request" });
    expect(link).toHaveAttribute("href", "/swimlanes/sitb/pull-requests");
  });

  it("underlines the link that sits in the job's line of text: it is told from the words around it by more than colour (axe `link-in-text-block`, found by the a11y run)", async () => {
    renderPrs(<PrJobView job={job(PrJobState.committing)} />);
    const link = await screen.findByRole("link", { name: "Find my pull request" });
    expect(link.className).toMatch(/(^|\s)underline(\s|$)/);
  });

  it("announces a change of step politely", async () => {
    renderPrs(<PrJobView job={job(PrJobState.committing)} />);
    expect(await screen.findByRole("list")).toHaveAttribute("aria-live", "polite");
  });

  it("names every step state in words, not only in a symbol", async () => {
    renderPrs(<PrJobView job={job(PrJobState.committing)} />);
    const items = await screen.findAllByRole("listitem");
    expect(within(items[0] as HTMLElement).getByLabelText("Done")).toBeInTheDocument();
    expect(within(items[1] as HTMLElement).getByLabelText("Working")).toBeInTheDocument();
    expect(within(items[2] as HTMLElement).getByLabelText("Waiting")).toBeInTheDocument();
  });

  it("never says a pull request was created, and shows no pull request, before the job is completed", async () => {
    for (const state of [PrJobState.pending, PrJobState.preparing, PrJobState.committing, PrJobState.opening]) {
      const { unmount } = renderPrs(<PrJobView job={job(state, { pr_url: URL419 })} />);
      await screen.findByText(/Waiting to start\.|Creating your pull request\./);
      expect(screen.queryByText(/created/i)).toBeNull();
      expect(screen.queryByRole("link", { name: /github\.com/ })).toBeNull();
      unmount();
    }
  });

  it("shows what the server wants the person to know about where the pull request goes", async () => {
    renderPrs(<PrJobView job={job(PrJobState.preparing, { warnings: ["A tenant file is copied into base."] })} />);
    expect(await screen.findByText("A tenant file is copied into base.")).toBeInTheDocument();
  });

  it("says when the job was last updated, from the server's time", async () => {
    renderPrs(<PrJobView job={job(PrJobState.preparing)} />);
    const line = await screen.findByText(/Job/);
    expect(line).toHaveTextContent("Job 7");
    expect(line).toHaveTextContent("1 file");
    expect(screen.getByTitle("2025-10-09 08:54 UTC")).toBeInTheDocument();
  });
});

describe("PrJobView, completed", () => {
  it("says the pull request was created, with its number and a link to it, only now", async () => {
    renderPrs(<PrJobView job={job(PrJobState.completed, { pr_url: URL419 })} effect="The file stays NFS ahead until the PR merges and sitb syncs." />);
    expect(await screen.findByText("PR #419 created.")).toBeInTheDocument();
    expect(screen.getByText("The file stays NFS ahead until the PR merges and sitb syncs.")).toBeInTheDocument();
    const link = screen.getByRole("link", { name: "github.com/csp-tenant/csp-tenant-data/pull/419" });
    expect(link).toHaveAttribute("href", URL419);
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getByRole("button", { name: "Copy PR link" })).toBeInTheDocument();
  });

  it("shows the badge the file will wear, as the design does", async () => {
    renderPrs(<PrJobView job={job(PrJobState.completed, { pr_url: URL419 })} />);
    expect(await screen.findByText(/Drift list now shows/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /PR #419 open/ })).toBeInTheDocument();
  });

  it("draws no steps once it is done", async () => {
    renderPrs(<PrJobView job={job(PrJobState.completed, { pr_url: URL419 })} />);
    await screen.findByText("PR #419 created.");
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("does not make up a link when the server gave none: it says so and points to the list", async () => {
    renderPrs(<PrJobView job={job(PrJobState.completed, { pr_url: null })} />);
    expect(await screen.findByText("The job finished, but the server gave no address that can be opened.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /github/ })).toBeNull();
    expect(screen.getByRole("link", { name: "Find my pull request" })).toBeInTheDocument();
  });

  it("does not link an address that is not https", async () => {
    renderPrs(<PrJobView job={job(PrJobState.completed, { pr_url: "javascript:alert(1)" })} />);
    expect(await screen.findByText("The job finished, but the server gave no address that can be opened.")).toBeInTheDocument();
    expect(document.body.innerHTML).not.toContain("javascript:");
  });
});

describe("PrJobView, failed", () => {
  it("says it failed, with what the server said as untrusted text, and draws no steps", async () => {
    renderPrs(<PrJobView job={job(PrJobState.failed, { error: "GitHub refused the push‮." })} />);
    expect(await screen.findByText("The pull request job failed.")).toBeInTheDocument();
    expect(screen.getByText(/GitHub refused the push/)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /U\+202E/i })).toBeInTheDocument();
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByText(/created/i)).toBeNull();
  });

  it("says to check GitHub for a branch before trying again", async () => {
    renderPrs(<PrJobView job={job(PrJobState.failed)} />);
    expect(await screen.findByText(/Check GitHub for a branch before you try again/)).toBeInTheDocument();
    expect(screen.queryByText("What the server said")).toBeNull();
  });

  it("is an alert, because the person has to act", async () => {
    renderPrs(<PrJobView job={job(PrJobState.failed)} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The pull request job failed.");
  });
});

describe("PrJobView, when the view may be behind", () => {
  it("says live updates are paused and offers to check now", async () => {
    const onCheck = vi.fn();
    renderPrs(<PrJobView job={job(PrJobState.committing)} behind={{ onCheck, checking: false }} />);
    expect(await screen.findByText(/Live updates are paused, so this may be behind the server/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Check now" }));
    expect(onCheck).toHaveBeenCalledOnce();
  });

  it("does not check twice while it is checking", async () => {
    const onCheck = vi.fn();
    renderPrs(<PrJobView job={job(PrJobState.committing)} behind={{ onCheck, checking: true }} />);
    const button = await screen.findByRole("button", { name: "Check now" });
    expect(button).toHaveAttribute("aria-busy", "true");
    await userEvent.click(button);
    expect(onCheck).not.toHaveBeenCalled();
  });

  it("says the last refresh failed, keeps the state it has, and offers to retry, with the request ID", async () => {
    const onRetry = vi.fn();
    renderPrs(<PrJobView job={job(PrJobState.committing)} refreshFailed={{ requestId: "req-77", onRetry }} />);
    expect(await screen.findByText(/Couldn't refresh this job\. Showing the last state we have/)).toBeInTheDocument();
    expect(screen.getByText("req-77")).toBeInTheDocument();
    expect(stepsOf()[1]).toBe("Committing:active");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });
});
