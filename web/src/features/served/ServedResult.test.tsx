import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ServedFile } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import type { QueryState } from "@/lib/query/use-query-state";
import { binaryServedFile, maskedServedFile, servedFile, withheldServedFile } from "@/mocks/design/served/data";
import { expectNoAxeViolations } from "@/test/axe";
import { ServedResult, type AgentFacts } from "./ServedResult";
import { renderServed } from "./test-utils";

// What the view shows for the answer to one request: the facts about it, the text (or why there is none), what the engine left over,
// and the states around it: loading, not served, an error with its request id, an answer that may be out of date. The text itself is
// components/served/ServedText; this is everything around it.

const ASKED = Date.now() - 3 * 60_000;

function ready(data: ServedFile, over: Partial<QueryState<ServedFile>> = {}): QueryState<ServedFile> {
  return { state: "ready", data, error: null, retryable: false, refreshing: false, stale: null, refetch: vi.fn(), ...over };
}
function failed(status: number, kind: string, over: Partial<QueryState<ServedFile>> = {}): QueryState<ServedFile> {
  const error = new ApiProblem({ status, kind, title: kind === "not_found" ? "Not found" : "Service unavailable", requestId: "req-served-1" });
  return { state: "error", data: undefined, error, retryable: status >= 500, refreshing: false, stale: null, refetch: vi.fn(), ...over };
}
const LOADING: QueryState<ServedFile> = { state: "loading", data: undefined, error: null, retryable: false, refreshing: false, stale: null, refetch: vi.fn() };

type Over = { state?: QueryState<ServedFile>; agent?: AgentFacts | null; fetching?: boolean; asOf?: number | null };
async function open({ state = ready(servedFile()), agent = { online: true }, fetching = false, asOf = ASKED }: Over = {}) {
  const view = renderServed(<ServedResult swimlane="sitb" state={state} agent={agent} fetching={fetching} asOf={asOf} />);
  await screen.findByRole("region", { name: "The config-server's answer" });
  return view;
}

describe("the room the answer takes (found by the accessibility run)", () => {
  it("scrolls the facts, the placeholders, the text and the sources as one when they do not fit, and gives the text a floor, so that long lists never leave it 0 px high", async () => {
    await open();
    const text = screen.getByRole("table", { name: "Served text of connection-pool.yml" });
    const answer = text.closest<HTMLElement>("[data-slot='served-answer']");
    if (answer === null) throw new Error("the answer has no scrolling box");
    expect(answer.className).toMatch(/(^|\s)overflow-y-auto(\s|$)/);
    expect(answer.className).toMatch(/(^|\s)min-h-0(\s|$)/);
    // The box that holds the lines (and the status bar under them) cannot shrink below a floor, so it is the others that scroll away.
    const lines = text.closest<HTMLElement>("[data-slot='served-text']");
    if (lines === null) throw new Error("the text has no box");
    expect(lines.className).toMatch(/(^|\s)min-h-48(\s|$)/);
    expect(lines.className).not.toMatch(/(^|\s)min-h-0(\s|$)/);
  });
});

describe("the facts about the answer", () => {
  it("names the file, the application, the tenant and that there was no channel, and what kind of file it is", async () => {
    await open();
    const region = screen.getByRole("region", { name: "The config-server's answer" });
    expect(within(region).getByText("tx-infinity-api/connection-pool.yml")).toBeInTheDocument();
    expect(region).toHaveTextContent("tx-infinity-core");
    expect(region).toHaveTextContent("sit7");
    expect(region).toHaveTextContent("no channel");
    expect(within(region).getByText("Resource file")).toBeInTheDocument();
  });

  it("names the channel when there is one", async () => {
    await open({ state: ready(servedFile({ channel: "beta" })) });
    expect(screen.getByRole("region", { name: "The config-server's answer" })).toHaveTextContent("channel beta");
  });

  it("says a property source is a property source", async () => {
    await open({ state: ready(servedFile({ role: "property_source" })) });
    expect(screen.getByText("Property source")).toBeInTheDocument();
  });

  it("gives the hash of what was served, with a copy button, and the file's path with one", async () => {
    await open();
    expect(screen.getByRole("button", { name: "Copy hash" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy path" })).toBeInTheDocument();
  });

  it("says when the config-server was asked, to the second, and how long ago", async () => {
    await open({ asOf: Date.UTC(2026, 9, 23, 14, 2, 41) });
    expect(screen.getByText(/Asked 2026-10-23 14:02:41 UTC/)).toBeInTheDocument();
  });

  it("makes the hidden characters of the file's name visible in the name of the text, too", async () => {
    await open({ state: ready(servedFile({ file: "x/inv\u202eoice.yml" })) });
    const table = screen.getByRole("table", { name: /Served text/ });
    expect(table.getAttribute("aria-label")).toContain("<U+202E>");
    expect(table.getAttribute("aria-label")).not.toContain("\u202e");
  });

  it("draws every value from the answer as text, with hidden characters made visible", async () => {
    await open({ state: ready(servedFile({ application: "a‮b", file: "x/<img src=x onerror=alert(1)>.yml" })) });
    expect(document.querySelector("img[src='x']")).toBeNull();
    expect(screen.getAllByRole("img", { name: /U\+202E/i }).length).toBeGreaterThan(0);
  });
});

describe("asking again", () => {
  it("is a button that asks the config-server again, and never changes what is asked", async () => {
    const state = ready(servedFile());
    await open({ state });
    await userEvent.click(screen.getByRole("button", { name: "Ask again" }));
    expect(state.refetch).toHaveBeenCalledTimes(1);
  });

  it("shows that it is asking, and a second press asks nothing", async () => {
    const state = ready(servedFile());
    await open({ state, fetching: true });
    const button = screen.getByRole("button", { name: "Asking…" });
    expect(button).toHaveAttribute("aria-busy", "true");
    await userEvent.click(button);
    expect(state.refetch).not.toHaveBeenCalled();
  });
});

describe("the text and what is left over", () => {
  it("draws the served text", async () => {
    await open();
    expect(screen.getByRole("table", { name: "Served text of connection-pool.yml" })).toBeInTheDocument();
  });

  it("draws a masked text with hidden values and no way to see them", async () => {
    await open({ state: ready(maskedServedFile()) });
    expect(document.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(screen.queryByRole("button", { name: /reveal/i })).toBeNull();
  });

  it("names the placeholders the config-server left, above the text", async () => {
    await open();
    expect(screen.getByText("1 placeholder left as written")).toBeInTheDocument();
  });

  it("says nothing about placeholders when none stayed", async () => {
    await open({ state: ready(servedFile({ unresolved_placeholders: [] })) });
    expect(screen.queryByText(/left as written/)).toBeNull();
  });

  it("lists the files the engine reads, as links to the file view", async () => {
    await open();
    const link = screen.getByRole("link", { name: "tx-infinity-api/connection-pool-sit7.yml" });
    expect(link).toHaveAttribute("href", `/swimlanes/sitb/files?path=${encodeURIComponent("tx-infinity-api/connection-pool-sit7.yml")}`);
  });

  it("says once that the comparison with the rendered view is not there yet, and offers no control for it", async () => {
    await open();
    expect(screen.getAllByText("Comparison with the rendered view is not available yet.")).toHaveLength(1);
    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.queryByRole("button", { name: /rendered|compare/i })).toBeNull();
    expect(screen.queryByRole("tab", { name: /rendered/i })).toBeNull();
  });
});

describe("a file the hub withholds", () => {
  it("shows no text, no placeholders, no sources, and no copy button for the hash", async () => {
    await open({ state: ready(withheldServedFile({ unresolved_placeholders: ["LEAK"], sources: [{ path: "security/keystore.jks", rank: 0 }] })) });
    expect(screen.getByText("Content withheld")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByText(/left as written/)).toBeNull();
    expect(screen.queryByRole("list", { name: /files the engine reads/i })).toBeNull();
    expect(screen.queryByRole("button", { name: "Copy hash" })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy path" })).toBeInTheDocument();
  });
});

describe("an answer that is not text", () => {
  it("says so, with the hash, and is not an error", async () => {
    await open({ state: ready(binaryServedFile()) });
    expect(screen.getByText("Not shown as text")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "Copy hash" })).toBeInTheDocument();
  });
});

describe("loading", () => {
  it("is a named, busy region with a footer that says what is loading", async () => {
    renderServed(<ServedResult swimlane="sitb" state={LOADING} agent={{ online: true }} fetching asOf={null} />);
    const group = await screen.findByRole("group", { name: "Loading the served file" });
    expect(group).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Loading the served file…")).toBeInTheDocument();
  });
});

describe("loading, in the dark", () => {
  it("draws its skeleton in the raised tone, because the muted tone is the card's own colour in the dark theme and the blocks cannot be seen (found by the T6 visual run: only the status line showed)", async () => {
    renderServed(<ServedResult swimlane="sitb" state={LOADING} agent={{ online: true }} fetching asOf={null} />);
    const group = await screen.findByRole("group", { name: "Loading the served file" });
    const blocks = group.querySelectorAll("[data-slot='skeleton']");
    expect(blocks.length).toBeGreaterThan(0);
    for (const block of blocks) {
      expect(block.className).toMatch(/(^|\s)bg-accent(\s|$)/);
      expect(block.className).not.toMatch(/(^|\s)bg-muted(\s|$)/);
    }
  });
});

describe("not served", () => {
  it("is an empty state, not an error: the config-server has nothing for this selection", async () => {
    renderServed(<ServedResult swimlane="sitb" state={failed(404, "not_found")} agent={{ online: true }} fetching={false} asOf={null} />);
    expect(await screen.findByText("Not served for this selection")).toBeInTheDocument();
    expect(screen.queryByText(/couldn't load/i)).toBeNull();
    expect(screen.getByText(/check the application and the channel/i)).toBeInTheDocument();
  });

  it("keeps the request id for a report, and asks again on request", async () => {
    const state = failed(404, "not_found");
    renderServed(<ServedResult swimlane="sitb" state={state} agent={{ online: true }} fetching={false} asOf={null} />);
    await screen.findByText("Not served for this selection");
    expect(screen.getByText("req-served-1")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Ask again" }));
    expect(state.refetch).toHaveBeenCalledTimes(1);
  });
});

describe("an error", () => {
  it("says what happened, shows the request id, and offers Retry when trying again can help", async () => {
    const state = failed(503, "service_unavailable");
    renderServed(<ServedResult swimlane="sitb" state={state} agent={{ online: true }} fetching={false} asOf={null} />);
    expect(await screen.findByText("Couldn't load the served file")).toBeInTheDocument();
    expect(screen.getByText("req-served-1")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(state.refetch).toHaveBeenCalledTimes(1);
  });

  it("says a refused read is a refused read, with no Retry for a 403", async () => {
    renderServed(<ServedResult swimlane="sitb" state={failed(403, "forbidden")} agent={{ online: true }} fetching={false} asOf={null} />);
    expect(await screen.findByText(/you do not have access/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });
});

describe("the agent", () => {
  it("says when the swimlane's agent is offline: the hub asks the config-server through it", async () => {
    await open({ agent: { online: false } });
    expect(screen.getByText("Agent for sitb is offline")).toBeInTheDocument();
    expect(screen.getByText(/asks the config-server through/i)).toBeInTheDocument();
  });

  it("says nothing when it is online, and nothing when it is not known yet", async () => {
    const { unmount } = await open({ agent: { online: true } });
    expect(screen.queryByText(/is offline/)).toBeNull();
    unmount();
    await open({ agent: null });
    expect(screen.queryByText(/is offline/)).toBeNull();
  });

  it("says it above an error too, because that is when it explains it", async () => {
    renderServed(<ServedResult swimlane="sitb" state={failed(503, "service_unavailable")} agent={{ online: false }} fetching={false} asOf={null} />);
    expect(await screen.findByText("Agent for sitb is offline")).toBeInTheDocument();
    expect(screen.getByText("Couldn't load the served file")).toBeInTheDocument();
  });
});

describe("an answer that may be out of date", () => {
  it("keeps the text and says it may be out of date when it is older than a minute", async () => {
    await open({ state: ready(servedFile(), { stale: { causes: ["age"], asOf: ASKED } }) });
    expect(screen.getByText("This view may be out of date")).toBeInTheDocument();
    expect(screen.getByRole("table", { name: /Served text/ })).toBeInTheDocument();
  });

  it("keeps the last answer and says the refresh failed, when it did", async () => {
    await open({ state: ready(servedFile(), { error: new ApiProblem({ status: 503, kind: "service_unavailable", title: "Service unavailable", requestId: "req-2" }), stale: { causes: ["refetch-failed"], asOf: ASKED } }) });
    expect(screen.getByText("Couldn't refresh")).toBeInTheDocument();
    expect(screen.getByRole("table", { name: /Served text/ })).toBeInTheDocument();
  });

  it("leaves 'live updates paused' to the shell, which says it once for the page; the time of the answer is always on screen", async () => {
    await open({ state: ready(servedFile(), { stale: { causes: ["live-paused"], asOf: ASKED } }) });
    expect(screen.queryByText("This view may be out of date")).toBeNull();
    expect(screen.queryByText(/live updates/i)).toBeNull();
    expect(screen.getByText(/Asked /)).toBeInTheDocument();
  });
});

describe("accessibility", () => {
  it.each([
    ["a text", ready(servedFile())],
    ["a masked text", ready(maskedServedFile())],
    ["a withheld file", ready(withheldServedFile())],
    ["a stale answer", ready(servedFile(), { stale: { causes: ["age"], asOf: ASKED } })],
    ["not served", failed(404, "not_found")],
    ["an error", failed(503, "service_unavailable")],
  ])("has no structural violation for %s", async (_name, state) => {
    renderServed(<ServedResult swimlane="sitb" state={state} agent={{ online: false }} fetching={false} asOf={ASKED} />);
    await screen.findByRole("region", { name: "The config-server's answer" });
    await expectNoAxeViolations();
  });
});
