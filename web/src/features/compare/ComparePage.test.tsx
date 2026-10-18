import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SwimlaneSummary } from "@/api/generated/model";
import { KeyboardProvider } from "@/keyboard";
import { createQueryClient } from "@/lib/query/client";
import { CompareScreen } from "./ComparePage";
import { gridServer, json, problem, row, stubHub, type Handler } from "./test-utils";

const lane = (id: string): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: [`t-${id}`],
  agent: "connected",
  last_scan_at: 0,
  base_version: null,
  drift_counts: [],
  finding_count: 0,
  tier: "SIT",
});
const LIST = ["sit", "sitb", "sitc"].map(lane);
const FILE = "tx-infinity-api/tx-infinity-core.yml";

function hub(extra: Handler = () => problem(404, "not_found")) {
  const grid = gridServer(2, ["sit", "sitb"], (i) => row(`a.item-${i}`, 2, "1", { 1: "2" }));
  const handler: Handler = (call) => {
    if (call.url.pathname === "/api/v1/swimlanes") return json({ items: LIST, next: null });
    if (call.url.pathname === "/api/v1/grid") return grid.handler(call);
    if (call.url.pathname === "/api/v1/compare/tree") return json({ entries: { items: [{ file: "a/b.yml", state: "different" }], next: null }, skipped_identical_subtrees: 3 });
    if (call.url.pathname === "/api/v1/compare") return json({ left: "effective:sit", right: "effective:sitb", file: "a/b.yml", class: "structured", identical: false, eol_only: false, hunks: [], setting_changes: [], binary: null });
    return extra(call);
  };
  return { handler, grid };
}

function show(search: Record<string, string> = {}) {
  const onAddress = vi.fn();
  const client = createQueryClient();
  const tree = (s: Record<string, string>) => (
    <QueryClientProvider client={client}>
      <KeyboardProvider>
        <CompareScreen search={s} onAddress={onAddress} />
      </KeyboardProvider>
    </QueryClientProvider>
  );
  const view = render(tree(search));
  return { onAddress, again: (s: Record<string, string>) => view.rerender(tree(s)) };
}

describe("the Compare page", () => {
  it("has its one heading at once, before any data, and the two modes", () => {
    stubHub(() => new Promise<Response>(() => undefined));
    show();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1, name: "Compare" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Compare mode" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "One file across swimlanes" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Whole swimlane" })).toHaveAttribute("aria-pressed", "false");
  });

  it("opens the grid for the file and the swimlanes of the address", async () => {
    const h = hub();
    stubHub(h.handler);
    show({ file: FILE, lanes: "sit,sitb", only: "all" });
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
    expect(h.grid.requests).toEqual([{ limit: 50, cursor: null }]);
    expect(screen.getByRole("switch", { name: "Only differing rows" })).toHaveAttribute("aria-checked", "false");
  });

  it("asks for a file when the address has none, and writes the file it is given to the address", async () => {
    stubHub(hub().handler);
    const user = userEvent.setup();
    const { onAddress } = show({ lanes: "sit,sitb" });
    await user.click(await screen.findByRole("button", { name: "Choose a file" }));
    await user.type(await screen.findByRole("textbox", { name: "Logical file" }), `${FILE}{Enter}`);
    await waitFor(() => expect(onAddress).toHaveBeenCalledWith(`/compare?file=${encodeURIComponent(FILE).replaceAll("%2F", "%2F")}&lanes=sit%2Csitb`));
  });

  it("writes the switch and the channel to the address, and leaves the defaults out", async () => {
    stubHub(hub().handler);
    const user = userEvent.setup();
    const { onAddress } = show({ file: FILE, lanes: "sit,sitb" });
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
    await user.click(screen.getByRole("switch", { name: "Only differing rows" }));
    expect(onAddress).toHaveBeenLastCalledWith(expect.stringContaining("only=all"));
    await user.click(screen.getByRole("switch", { name: "Only differing rows" }));
    expect(onAddress).toHaveBeenLastCalledWith(expect.not.stringContaining("only="));
  });

  it("takes the address when it changes by itself (Back, a link)", async () => {
    stubHub(hub().handler);
    const { again } = show({ file: FILE, lanes: "sit,sitb" });
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
    again({ file: FILE, lanes: "sit,sitb", only: "all" });
    await waitFor(() => expect(screen.getByRole("switch", { name: "Only differing rows" })).toHaveAttribute("aria-checked", "false"));
  });

  it("switches to the whole-swimlane compare of the first two swimlanes, and back to the grid", async () => {
    const h = hub();
    const calls = stubHub(h.handler);
    const user = userEvent.setup();
    const { onAddress } = show({ file: FILE, lanes: "sitb,sitc" });
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
    await user.click(screen.getByRole("button", { name: "Whole swimlane" }));
    expect(screen.getByRole("button", { name: "Whole swimlane" })).toHaveAttribute("aria-pressed", "true");
    await screen.findByRole("tree", { name: "Differing files" });
    const tree = calls.find((c) => c.url.pathname === "/api/v1/compare/tree");
    expect(tree?.url.searchParams.get("left")).toBe("effective:sitb");
    expect(tree?.url.searchParams.get("right")).toBe("effective:sitc");
    expect(screen.getByText(/Every directory and logical file in one swimlane/)).toBeInTheDocument();
    expect(onAddress).toHaveBeenLastCalledWith(expect.stringContaining("/compare"));
    await user.click(screen.getByRole("button", { name: "One file across swimlanes" }));
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
  });

  it("opens as the whole-swimlane compare when the address names the sides, as the server's links do", async () => {
    const calls = stubHub(hub().handler);
    show({ left: "nfs:sitb", right: "git:tenant@sit7", file: "a/b.yml" });
    await screen.findByRole("tree", { name: "Differing files" });
    const tree = calls.find((c) => c.url.pathname === "/api/v1/compare/tree");
    expect(tree?.url.searchParams.get("left")).toBe("nfs:sitb");
    expect(tree?.url.searchParams.get("right")).toBe("git:tenant@sit7");
    expect(screen.getByRole("button", { name: "Whole swimlane" })).toHaveAttribute("aria-pressed", "true");
  });

  it("ignores what is not valid in the address instead of sending it to the server", async () => {
    const h = hub();
    stubHub(h.handler);
    show({ file: "../../etc/passwd", lanes: "sit,Not A Lane,sitb", channel: "a b" });
    expect(await screen.findByText("Choose a file to compare")).toBeInTheDocument();
    expect(h.grid.requests).toHaveLength(0);
  });
});
