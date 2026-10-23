import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent, PrLink } from "@/api/generated/model";
import { expectNoAxeViolations } from "@/test/axe";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";
import PullRequestsPage from "./PullRequestsPage";
import { prLink, problem, renderPrs, routes, type Call } from "./test-utils";

// The Pull requests page of a swimlane (the page `LinkTemplates::default().pr` sends people to, and where "Find my pull request"
// goes): one row for each link of the swimlane, filtered by state, newest first, live. There is no design frame for it
// (H2): it is built from the Approvals page's shape and the handoff's badges, and it is a deviation row.


const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const OTHER = "99999999-9999-4999-8999-999999999999:99999999-9999-4999-8999-999999999999";
const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";

const OPEN = prLink({ id: 12, pr_number: 418, author: ME });
const MERGED = prLink({ id: 11, state: "merged", pr_number: 417, path: "application.properties", author: OTHER, closed_at: 1_760_000_000_000 - 3_600_000 });
const CLOSED = prLink({ id: 10, state: "closed", pr_number: 409, path: "limits/limits.yml", author: OTHER, closed_at: 1_760_000_000_000 - 86_400_000 });

const query = (call: Call) => new URL(call.path, "http://hub.test").searchParams;
const page = (links: PrLink[], next: string | null = null) => ({ items: links, next });
/** A hub that filters the way the server does: by state when it is asked, newest (highest id) first. */
const hub = (all: () => PrLink[]) => routes({ "GET /api/v1/prs": (call) => page(all().filter((l) => query(call).get("state") === null || l.state === query(call).get("state")).sort((a, b) => b.id - a.id)) });

const render = (search: Record<string, string> = {}, api = hub(() => [OPEN, MERGED, CLOSED]), options: Parameters<typeof renderPrs>[1] = {}) =>
  renderPrs(<PullRequestsPage params={{ id: "sitb" }} search={search} />, { api, user: { id: ME }, ...options });

describe("the page", () => {
  it("has its one h1 as soon as it is drawn, before its data arrives", async () => {
    const held = new Promise<Response>(() => undefined);
    renderPrs(<PullRequestsPage params={{ id: "sitb" }} search={{}} />, { api: () => held });
    expect(await screen.findByRole("heading", { level: 1, name: "Pull requests" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(await screen.findByRole("group", { name: "Loading pull requests" })).toBeInTheDocument();
  });

  it("names the swimlane and links back to it", async () => {
    render();
    await screen.findByRole("grid", { name: "Pull requests" });
    const link = screen.getByRole("link", { name: "sitb" });
    expect(link).toHaveAttribute("href", "/swimlanes/sitb");
  });

  it("asks for the open pull requests of the swimlane by default, 100 at a time", async () => {
    const { calls } = render();
    await screen.findByRole("grid", { name: "Pull requests" });
    const asked = calls.filter((c) => c.path.startsWith("/api/v1/prs")).map((c) => Object.fromEntries(query(c)));
    expect(asked).toEqual([{ swimlane: "sitb", state: "open", limit: "100" }]);
  });

  it.each([
    [{ state: "merged" }, { swimlane: "sitb", state: "merged", limit: "100" }],
    [{ state: "closed" }, { swimlane: "sitb", state: "closed", limit: "100" }],
    [{ state: "all" }, { swimlane: "sitb", limit: "100" }],
    [{ state: "nonsense" }, { swimlane: "sitb", state: "open", limit: "100" }],
  ])("reads the filter %j from the address", async (search, want) => {
    const { calls } = render(search);
    await screen.findByRole("grid", { name: "Pull requests" });
    expect(calls.filter((c) => c.path.startsWith("/api/v1/prs")).map((c) => Object.fromEntries(query(c)))).toEqual([want]);
  });

  it("marks the filter in force, and offers the other three", async () => {
    render({ state: "merged" });
    await screen.findByRole("grid", { name: "Pull requests" });
    const group = screen.getByRole("radiogroup", { name: "State" });
    expect(within(group).getByRole("radio", { name: "Merged" })).toBeChecked();
    for (const name of ["Open", "Closed", "All"]) expect(within(group).getByRole("radio", { name })).not.toBeChecked();
  });
});

describe("the rows", () => {
  it("says the state with an icon and a word, never a colour alone", async () => {
    render({ state: "all" });
    const grid = await screen.findByRole("grid", { name: "Pull requests" });
    const rows = within(grid).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    expect(within(rows[0] as HTMLElement).getByText("Open")).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("Merged")).toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).getByText("Closed")).toBeInTheDocument();
    for (const row of rows) expect(row.querySelector("[data-slot='pr-state'] svg")).not.toBeNull();
  });

  it("links each number to its pull request on GitHub, in another tab", async () => {
    render({ state: "all" });
    const link = await screen.findByRole("link", { name: /#418/ });
    expect(link).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
  });

  it("links each file to the file in the app", async () => {
    render();
    const link = await screen.findByRole("link", { name: PATH });
    expect(link).toHaveAttribute("href", `/swimlanes/sitb/files?path=${encodeURIComponent(PATH)}`);
  });

  it("says whose it is: yours, or Lanekeeper user and the start of the id", async () => {
    render({ state: "all" });
    const grid = await screen.findByRole("grid", { name: "Pull requests" });
    const rows = within(grid).getAllByRole("row").slice(1);
    expect(within(rows[0] as HTMLElement).getByText("You")).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("Lanekeeper user 99999999")).toBeInTheDocument();
  });

  it("draws a link whose address is not https as its number only", async () => {
    render({}, hub(() => [prLink({ pr_url: "javascript:alert(1)" })]));
    const grid = await screen.findByRole("grid", { name: "Pull requests" });
    expect(within(grid).queryByRole("link", { name: /#418/ })).toBeNull();
    expect(within(grid).getByText("#418")).toBeInTheDocument();
    expect(document.body.innerHTML).not.toContain("javascript:");
  });

  it("draws a path with a hidden control character as a visible marker", async () => {
    render({}, hub(() => [prLink({ path: "a/b‮c.yml" })]));
    const grid = await screen.findByRole("grid", { name: "Pull requests" });
    expect(within(grid).getByRole("img", { name: /U\+202E/i })).toBeInTheDocument();
  });

  it("opens the pull request when Enter is pressed on its row", async () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    render();
    const grid = await screen.findByRole("grid", { name: "Pull requests" });
    grid.focus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(click).toHaveBeenCalled());
    const anchor = click.mock.contexts[0] as HTMLAnchorElement;
    expect(anchor.getAttribute("href")).toBe("https://github.com/csp/csp-tenant-data/pull/418");
    click.mockRestore();
  });

  it("has no structural accessibility violation", async () => {
    render({ state: "all" });
    await screen.findByRole("grid", { name: "Pull requests" });
    await expectNoAxeViolations();
  });
});

describe("the other states of the list", () => {
  it("says there are no open pull requests, and where one will appear", async () => {
    render({}, hub(() => []));
    expect(await screen.findByText("No open pull requests")).toBeInTheDocument();
    expect(screen.getByText(/A pull request raised from a file of sitb appears here as it happens/)).toBeInTheDocument();
  });

  it("names the filter in the empty state of another filter", async () => {
    render({ state: "merged" }, hub(() => []));
    expect(await screen.findByText("No merged pull requests")).toBeInTheDocument();
  });

  it("says there are none at all for 'all'", async () => {
    render({ state: "all" }, hub(() => []));
    expect(await screen.findByText("No pull requests yet")).toBeInTheDocument();
  });

  it("shows the problem's title and the request ID when the list cannot be read, and Retry reads it again", async () => {
    let failing = true;
    const { calls } = render({}, (call) => (failing ? problem(500, "internal") : hub(() => [OPEN])(call)));
    expect(await screen.findByText("Couldn't load pull requests", {}, { timeout: 8000 })).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    failing = false;
    const before = calls.length;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("grid", { name: "Pull requests" })).toBeInTheDocument();
    expect(calls.length).toBeGreaterThan(before);
  });

  it("loads the next page when asked, and says how many are shown so far", async () => {
    const second = prLink({ id: 3, pr_number: 300, path: "old.yml" });
    const api = routes({ "GET /api/v1/prs": (call) => (query(call).get("cursor") === null ? page([OPEN], "next-1") : page([second])) });
    const { calls } = render({}, api);
    await screen.findByRole("grid", { name: "Pull requests" });
    expect(screen.getByText("Showing 1 so far")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByRole("link", { name: /#300/ })).toBeInTheDocument();
    expect(calls.some((c) => query(c).get("cursor") === "next-1")).toBe(true);
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });
});

describe("live updates", () => {
  let server: FakeHub;
  let links: PrLink[];
  let nextId: number;
  beforeEach(() => {
    server = new FakeHub();
    links = [OPEN];
    nextId = 41;
  });

  const send = async (event: DomainEvent): Promise<void> => {
    await act(async () => {
      server.last.push(frame(nextId++, event));
      await Promise.resolve();
    });
  };

  async function open(search: Record<string, string> = {}) {
    const app = render(search, hub(() => links), { live: { stream: { fetch: server.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
    await screen.findByRole("grid", { name: "Pull requests" });
    await waitFor(() => expect(server.connections).toHaveLength(1), { timeout: 4000 });
    await act(async () => {
      server.last.push(FIRST_FRAME);
      await Promise.resolve();
    });
    await new Promise((resolve) => setTimeout(resolve, 250));
    return { ...app, listCalls: () => app.calls.filter((c) => c.path.startsWith("/api/v1/prs")).length };
  }

  it("updates a row in place when pr_state_changed names a link the list already has", async () => {
    const { listCalls } = await open({ state: "all" });
    const before = listCalls();
    links = [{ ...OPEN, state: "merged", closed_at: 1_760_000_000_000 }];
    await send({ type: "pr_state_changed", link_id: 12, state: "merged" });
    await waitFor(() => expect(listCalls()).toBe(before + 1), { timeout: 4000 });
    const grid = screen.getByRole("grid", { name: "Pull requests" });
    await waitFor(() => expect(within(grid).getByText("Merged")).toBeInTheDocument());
    expect(within(grid).queryByText("Open")).toBeNull();
    expect(screen.queryByText(/updates?$/)).toBeNull();
  });

  it("holds the list still when a row would leave it, counts the change, and shows it when asked", async () => {
    await open();
    expect(screen.getByRole("link", { name: /#418/ })).toBeInTheDocument();
    links = [{ ...OPEN, state: "merged", closed_at: 1_760_000_000_000 }];
    await send({ type: "pr_state_changed", link_id: 12, state: "merged" });
    expect(await screen.findByText("1 update", {}, { timeout: 4000 })).toBeInTheDocument();
    // Still there: the list does not move under the reader.
    expect(screen.getByRole("link", { name: /#418/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    await waitFor(() => expect(screen.queryByRole("link", { name: /#418/ })).toBeNull());
    expect(screen.getByText("No open pull requests")).toBeInTheDocument();
  });

  it("holds a new pull request back the same way", async () => {
    await open();
    links = [prLink({ id: 13, pr_number: 430, path: "new.yml" }), OPEN];
    await send({ type: "pr_state_changed", link_id: 13, state: "open" });
    expect(await screen.findByText("1 update", {}, { timeout: 4000 })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /#430/ })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    expect(await screen.findByRole("link", { name: /#430/ })).toBeInTheDocument();
  });
});

