import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import { STATES } from "@/handoff/states.generated";
import type { QueryState } from "@/lib/query/use-query-state";
import { expectNoAxeViolations } from "@/test/axe";
import { ListSkeleton, QueryBoundary, QueryBoundaryView } from "./QueryBoundary";

// The five states of a query on screen (design: States index, rows L, E, R, U and the "data as of" stamp of O), as one
// component: loading, error, empty, ready, and the two overlays that never replace content: refreshing and stale.

const AS_OF = Date.UTC(2026, 9, 17, 10, 42, 0);
const NOW = AS_OF + 4 * 60_000;

type D = { items: string[] };
const view = (patch: Partial<QueryState<D>> = {}): QueryState<D> => ({
  state: "ready",
  data: { items: ["a", "b"] },
  error: null,
  retryable: false,
  refreshing: false,
  stale: null,
  refetch: () => undefined,
  ...patch,
});
const problem = (status: number, kind = "x", extra: { requestId?: string } = {}) => new ApiProblem({ status, kind, title: `Title ${status}`, ...extra });

/** The stale banner (an Alert), whatever else on the screen is a live region. */
const banner = (): HTMLElement => {
  const found = document.querySelector<HTMLElement>("[data-slot='alert']");
  if (!found) throw new Error("no banner on screen");
  return found;
};

function show(state: QueryState<D>, props: { staleBanner?: boolean; empty?: ReactNode; loading?: ReactNode } = {}) {
  return render(
    <QueryBoundaryView state={state} noun="swimlanes" source="the swimlane list" now={NOW} {...props}>
      {(data) => <ul>{data.items.map((i) => <li key={i}>{i}</li>)}</ul>}
    </QueryBoundaryView>,
  );
}

describe("loading (design L: skeletons, and a footer that names what is loading)", () => {
  it("is a busy region with a name, a skeleton, and the footer, and shows no data", () => {
    show(view({ state: "loading", data: undefined }));
    const region = screen.getByLabelText("Loading swimlanes");
    expect(region).toHaveAttribute("aria-busy", "true");
    expect(region.querySelector("[data-slot='skeleton']")).not.toBeNull();
    expect(screen.getByText("Loading swimlanes…").closest("[role='status']")).not.toBeNull();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
  });

  it("is a group, because `aria-label` is not allowed on an element with no role (axe `aria-prohibited-attr`)", () => {
    show(view({ state: "loading", data: undefined }));
    const region = screen.getByRole("group", { name: "Loading swimlanes" });
    expect(region).toHaveAttribute("aria-busy", "true");
    // The footer is the one live region: a status inside a status would be said twice.
    expect(region.closest("[role='status']")).toBeNull();
  });

  it("lets the screen draw its own skeleton, with its own grid, instead of the default", () => {
    show(view({ state: "loading", data: undefined }), { loading: <div data-testid="mine" /> });
    expect(screen.getByTestId("mine")).toBeInTheDocument();
    expect(document.querySelector("[data-slot='list-skeleton']")).toBeNull();
  });

  it("keeps the default skeleton at a fixed row height, so nothing moves when the data arrives", () => {
    show(view({ state: "loading", data: undefined }));
    const rows = document.querySelectorAll("[data-slot='list-skeleton'] > div");
    expect(rows.length).toBe(7);
    for (const row of rows) expect(row.className).toContain("h-8");
  });
});

describe("error (design R: the problem's title, one line, the request ID with copy, Retry as the only action)", () => {
  it("shows the title, the request ID with a copy button, and the one-line explanation", () => {
    show(view({ state: "error", data: undefined, error: problem(503, "x", { requestId: "req-abc-1" }), retryable: true }));
    const alert = screen.getByRole("alert");
    expect(within(alert).getByRole("heading", { name: "Couldn't load swimlanes" })).toBeInTheDocument();
    expect(alert).toHaveTextContent("Title 503");
    expect(alert).toHaveTextContent("The server returned an error while reading the swimlane list. Retry, or send the request ID to the platform team.");
    expect(alert).toHaveTextContent("req-abc-1");
    expect(within(alert).getByRole("button", { name: "Copy request ID" })).toBeInTheDocument();
  });

  it("offers Retry for a problem that retrying can change, and asks for the refetch when it is pressed", async () => {
    const refetch = vi.fn();
    show(view({ state: "error", data: undefined, error: problem(503), retryable: true, refetch }));
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it.each([
    [403, /do not have access to the swimlane list/i],
    [404, /the swimlane list was not found/i],
    [400, /could not be read: the request was refused/i],
  ])("offers no Retry for a %i, and says what it means", (status, text) => {
    show(view({ state: "error", data: undefined, error: problem(status), retryable: false }));
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(text);
    expect(screen.getByRole("alert")).not.toHaveTextContent(/Retry, or send/);
  });

  it("says the server could not be reached when nothing came back", () => {
    show(view({ state: "error", data: undefined, error: problem(0, "network"), retryable: true }));
    expect(screen.getByRole("alert")).toHaveTextContent(/could not be reached while reading the swimlane list/i);
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("never shows the detail the server wrote, only the title and the request ID (S21)", () => {
    const withDetail = new ApiProblem({ status: 500, kind: "internal", title: "Server error", detail: "stack: secret at /srv/x", requestId: "req-1" });
    show(view({ state: "error", data: undefined, error: withDetail, retryable: true }));
    expect(screen.getByRole("alert")).not.toHaveTextContent("secret");
  });

  it("omits the request ID line when the problem has none", () => {
    show(view({ state: "error", data: undefined, error: problem(503), retryable: true }));
    expect(screen.queryByRole("button", { name: "Copy request ID" })).not.toBeInTheDocument();
  });
});

describe("empty (design E: what is missing and the next step)", () => {
  it("shows the screen's own empty state, and no data, no banner", () => {
    show(view({ state: "empty", data: { items: [] } }), { empty: <p>No swimlanes yet. Ask an admin to register one.</p> });
    expect(screen.getByText("No swimlanes yet. Ask an admin to register one.")).toBeInTheDocument();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
  });

  it("has a plain default that names the noun, for a screen that has not drawn one", () => {
    show(view({ state: "empty", data: { items: [] } }));
    expect(screen.getByText(/No swimlanes/)).toBeInTheDocument();
  });

  it("keeps the stale banner over an empty list: an old empty list is the one most worth refreshing", () => {
    show(view({ state: "empty", data: { items: [] }, stale: { causes: ["age"], asOf: AS_OF } }), { empty: <p>none</p> });
    expect(banner()).toHaveTextContent("This view may be out of date");
    expect(screen.getByText("none")).toBeInTheDocument();
  });
});

describe("ready", () => {
  it("renders the screen's content with the data, and shows no overlay", () => {
    show(view());
    expect(screen.getAllByRole("listitem").map((li) => li.textContent)).toEqual(["a", "b"]);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText(/out of date|paused|refresh/i)).not.toBeInTheDocument();
  });
});

describe("refreshing: a background refetch is a small indicator, not a skeleton", () => {
  it("keeps a place for the indicator whether or not it shows, so it never moves the content", () => {
    const { container, rerender } = show(view());
    const slot = () => container.querySelector("[data-slot='refreshing']");
    expect(slot()).not.toBeNull();
    expect(slot()?.className).toContain("absolute");
    expect(slot()?.childElementCount).toBe(0);
    rerender(
      <QueryBoundaryView state={view({ refreshing: true })} noun="swimlanes" source="the swimlane list" now={NOW}>
        {(data) => <ul>{data.items.map((i) => <li key={i}>{i}</li>)}</ul>}
      </QueryBoundaryView>,
    );
    expect(slot()).not.toBeNull();
    expect(slot()?.querySelector("svg")).not.toBeNull();
  });

  it("marks the region busy and announces it politely, and keeps showing the data", () => {
    const { container } = show(view({ refreshing: true }));
    expect(container.querySelector("[data-slot='query-boundary']")).toHaveAttribute("aria-busy", "true");
    const slot = container.querySelector("[data-slot='refreshing']");
    expect(slot).toHaveAttribute("role", "status");
    expect(slot).toHaveTextContent("Refreshing");
    expect(document.querySelector("[data-slot='alert']")).toBeNull();
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    expect(document.querySelector("[data-slot='skeleton']")).toBeNull();
  });

  it("is not busy when it is not refreshing", () => {
    const { container } = show(view());
    expect(container.querySelector("[data-slot='query-boundary']")).not.toHaveAttribute("aria-busy");
  });

  it("spins with the app's own utility, which the reduced-motion rule of the base styles stops", () => {
    const { container } = show(view({ refreshing: true }));
    expect(container.querySelector("[data-slot='refreshing'] svg")?.getAttribute("class")).toContain("animate-lk-spin");
  });
});

describe("stale: a banner that names the cause and when the data is from (design O and U)", () => {
  it("age: may be out of date, with the time and how long ago", () => {
    show(view({ stale: { causes: ["age"], asOf: AS_OF } }));
    expect(banner()).toHaveTextContent("This view may be out of date");
    expect(banner()).toHaveTextContent("Last updated 10:42 UTC (4 min ago)");
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });

  it("live-paused: says nothing, because the shell already says it once for the whole page (a screen with three lists would say it four times)", () => {
    show(view({ stale: { causes: ["live-paused"], asOf: AS_OF } }));
    expect(document.querySelector("[data-slot='alert']")).toBeNull();
    expect(screen.queryByText(/Live updates/)).toBeNull();
  });

  it("refetch-failed: the data on screen is the last good copy, and the error is said once, without an error panel", () => {
    show(view({ error: problem(503, "x", { requestId: "req-9" }), retryable: true, stale: { causes: ["refetch-failed"], asOf: AS_OF } }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    const b = banner();
    expect(b).toHaveTextContent("Couldn't refresh");
    expect(b).toHaveTextContent("Showing the last data that loaded");
    expect(b).toHaveTextContent("Title 503");
    expect(b).toHaveTextContent("Last updated 10:42 UTC");
  });

  it("names the request ID of the refresh that failed, so that it can be reported, and says nothing of one the server did not give (found by the T6 visual run: it was lost)", () => {
    show(view({ error: problem(503, "x", { requestId: "req-9" }), retryable: true, stale: { causes: ["refetch-failed"], asOf: AS_OF } }));
    expect(banner()).toHaveTextContent("Request ID");
    expect(banner()).toHaveTextContent("req-9");
    expect(within(banner()).getByRole("button", { name: /copy request id/i })).toBeInTheDocument();
  });

  it("has no request ID line when the refresh failed with no answer, or the data is only old", () => {
    show(view({ error: problem(503, "x"), retryable: true, stale: { causes: ["refetch-failed"], asOf: AS_OF } }));
    expect(banner()).not.toHaveTextContent("Request ID");
    show(view({ stale: { causes: ["age"], asOf: AS_OF } }));
    expect(screen.queryByText("Request ID")).toBeNull();
  });

  it("says its own cause when the stream is also down, and does not repeat what the shell says about the stream", () => {
    show(view({ error: problem(500), stale: { causes: ["refetch-failed", "live-paused"], asOf: AS_OF } }));
    const b = banner();
    expect(b).toHaveTextContent("Couldn't refresh");
    expect(b).not.toHaveTextContent(/live updates/i);
    show(view({ stale: { causes: ["live-paused", "age"], asOf: AS_OF } }));
    expect(screen.getAllByText("This view may be out of date").length).toBeGreaterThan(0);
    expect(screen.queryByText("Live updates paused")).toBeNull();
  });

  it("says every cause of its own, with the most urgent as the title", () => {
    show(view({ error: problem(500), stale: { causes: ["refetch-failed", "age"], asOf: AS_OF } }));
    expect(banner()).toHaveTextContent("Couldn't refresh");
  });

  it("copes with a time it does not know", () => {
    show(view({ stale: { causes: ["age"], asOf: null } }));
    expect(banner()).not.toHaveTextContent(/Last updated/);
    expect(banner()).toHaveTextContent("This view may be out of date");
  });

  it("has a Refresh button that asks for the refetch", async () => {
    const refetch = vi.fn();
    show(view({ stale: { causes: ["age"], asOf: AS_OF }, refetch }));
    await userEvent.setup().click(screen.getByRole("button", { name: "Refresh" }));
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it("can be switched off by a screen that says the same thing elsewhere", () => {
    show(view({ stale: { causes: ["age"], asOf: AS_OF } }), { staleBanner: false });
    expect(document.querySelector("[data-slot='alert']")).toBeNull();
  });
});

describe("the design's state vocabulary (src/handoff/states.generated.ts, read from the States index)", () => {
  const section = (code: string) => STATES.sections.find((s) => s.code === code);

  it("covers every state the index lists for a screen's data: Loading, Empty, Error, Live updates paused, and the 'data as of' stamp", () => {
    expect(section("L")?.title).toBe("Loading");
    expect(section("E")?.title).toBe("Empty");
    expect(section("R")?.title).toBe("Error");
    expect(section("U")?.title).toBe("Live updates paused");
    expect(section("O")?.desc).toMatch(/data as of/i);
    // The error row: "Problem title, one-line explanation, request ID with copy."
    expect(section("R")?.desc).toBe("Problem title, one-line explanation, request ID with copy.");
    show(view({ state: "error", data: undefined, error: problem(503, "x", { requestId: "req-1" }), retryable: true }));
    const alert = screen.getByRole("alert");
    expect(within(alert).getByRole("heading")).toBeInTheDocument(); // the title
    expect(alert).toHaveTextContent("Title 503"); // the problem's title
    expect(alert.querySelectorAll("p").length).toBeGreaterThanOrEqual(1); // one line
    expect(within(alert).getByRole("button", { name: "Copy request ID" })).toBeInTheDocument();
  });

  it("stamps the data with the time it is from, as the offline rows do", () => {
    show(view({ stale: { causes: ["age"], asOf: AS_OF } }));
    expect(banner()).toHaveTextContent(/10:42 UTC/);
  });
});

describe("accessibility", () => {
  it.each([
    ["loading", view({ state: "loading", data: undefined })],
    ["error", view({ state: "error", data: undefined, error: problem(503, "x", { requestId: "req-1" }), retryable: true })],
    ["error, not retryable", view({ state: "error", data: undefined, error: problem(404), retryable: false })],
    ["empty", view({ state: "empty", data: { items: [] } })],
    ["ready", view()],
    ["refreshing", view({ refreshing: true })],
    ["stale", view({ stale: { causes: ["age", "live-paused"], asOf: AS_OF } })],
    ["refetch failed", view({ error: problem(503), stale: { causes: ["refetch-failed"], asOf: AS_OF } })],
  ])("%s has no violations", async (_name, state) => {
    show(state);
    await expectNoAxeViolations();
  });
});

describe("connected to a query", () => {
  function Connected({ fn }: { fn: () => Promise<D> }) {
    const query = useQuery<D, ApiProblem>({ queryKey: ["zz-boundary"], queryFn: fn, retry: false });
    return (
      <QueryBoundary query={query} noun="swimlanes" source="the swimlane list" isEmpty={(d) => d.items.length === 0} empty={<p>nothing here</p>}>
        {(data) => <ul>{data.items.map((i) => <li key={i}>{i}</li>)}</ul>}
      </QueryBoundary>
    );
  }
  const wrap = (ui: ReactNode) => render(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);

  it("goes loading, then ready", async () => {
    wrap(<Connected fn={() => Promise.resolve({ items: ["x"] })} />);
    expect(screen.getByLabelText("Loading swimlanes")).toBeInTheDocument();
    expect(await screen.findByText("x")).toBeInTheDocument();
    expect(screen.queryByLabelText("Loading swimlanes")).not.toBeInTheDocument();
  });

  it("goes loading, then empty", async () => {
    wrap(<Connected fn={() => Promise.resolve({ items: [] })} />);
    expect(await screen.findByText("nothing here")).toBeInTheDocument();
  });

  it("goes loading, then error, and Retry loads it", async () => {
    let attempt = 0;
    wrap(
      <Connected
        fn={() => {
          attempt += 1;
          return attempt === 1 ? Promise.reject(problem(503, "x", { requestId: "req-1" })) : Promise.resolve({ items: ["ok"] });
        }}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't load swimlanes");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("ok")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  });
});

describe("ListSkeleton: the blocks' own look", () => {
  const blocks = (container: HTMLElement) => Array.from(container.querySelectorAll<HTMLElement>("[data-slot='skeleton']"));

  it("gives every block the class a surface asks for (a card in the dark theme is the colour of `muted`), and none otherwise", () => {
    const { container, rerender } = render(<ListSkeleton rows={2} columns={3} />);
    expect(blocks(container)).toHaveLength(6);
    expect(blocks(container).filter((b) => b.classList.contains("bg-accent"))).toHaveLength(0);
    rerender(<ListSkeleton rows={2} columns={3} blockClassName="bg-accent" />);
    expect(blocks(container).filter((b) => b.classList.contains("bg-accent"))).toHaveLength(6);
  });
});
