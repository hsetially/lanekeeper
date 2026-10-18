import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, me, problem, renderApp, type Api } from "@/test/app";
import { gridServer, row } from "./test-utils";

// The Compare page in the real app: the route table, the router's own search handling, the session and the shell around it.
// What this proves that the other tests cannot: the page is the route's default export, it gets the address the router parsed,
// and the address it writes comes back to it without a loop.

const FILE = "tx-infinity-api/tx-infinity-core.yml";
const LANES = ["sit", "sitb"];

function api(): { api: Api; grid: ReturnType<typeof gridServer> } {
  const grid = gridServer(2, LANES, (i) => row(`a.item-${i}`, 2, "1", { 1: "2" }));
  const lanes = LANES.map((id) => ({ id, display_name: id, tenants: [`t-${id}`], agent: "connected", last_scan_at: 0, base_version: null, drift_counts: [], finding_count: 0, tier: "SIT" }));
  return {
    grid,
    api: (call) => {
      const path = new URL(call.path, "http://hub.test").pathname;
      if (path === "/api/v1/me") return json(me("viewer"));
      if (path === "/api/v1/swimlanes") return json({ items: lanes, next: null });
      if (path === "/api/v1/grid") return grid.handler({ url: new URL(call.path, "http://hub.test"), method: call.method, headers: call.headers });
      return problem(404, "not_found");
    },
  };
}

describe("Compare in the app", () => {
  it("opens the grid for the address, and keeps the address in step with the switch", async () => {
    const h = api();
    const user = userEvent.setup();
    const app = renderApp(`/compare?file=${encodeURIComponent(FILE)}&lanes=sit,sitb`, h.api);
    expect(await screen.findByRole("heading", { level: 1, name: "Compare" }, { timeout: 10_000 })).toBeInTheDocument();
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" }, { timeout: 10_000 });
    expect(h.grid.requests).toEqual([{ limit: 50, cursor: null }]);

    await user.click(screen.getByRole("switch", { name: "Only differing rows" }));
    await waitFor(() => expect(app.location()).toContain("only=all"));
    expect(app.location()).toContain(`file=${encodeURIComponent(FILE)}`);
    // The address came back to the page and did not start anything: one more grid read for the new filter, and no more.
    await waitFor(() => expect(screen.getByRole("switch", { name: "Only differing rows" })).toHaveAttribute("aria-checked", "false"));
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(h.grid.requests).toHaveLength(2);

    await user.click(screen.getByRole("switch", { name: "Only differing rows" }));
    await waitFor(() => expect(app.location()).not.toContain("only="));
  }, 30_000);

  it("opens for a Viewer with no file chosen, and asks for one", async () => {
    const h = api();
    renderApp("/compare", h.api);
    expect(await screen.findByRole("heading", { level: 1, name: "Compare" }, { timeout: 10_000 })).toBeInTheDocument();
    expect(await screen.findByText("Choose a file to compare", undefined, { timeout: 10_000 })).toBeInTheDocument();
  }, 30_000);
});
