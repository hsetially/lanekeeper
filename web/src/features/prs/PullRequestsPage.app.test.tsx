import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, me, renderApp, type Call } from "@/test/app";
import { prLink } from "./test-utils";

// The page inside the real app: the route table, the router, the session and the shell. The route is `/swimlanes/:id/pull-requests`
// (the address `LinkTemplates::default().pr` sends people to), open to a Viewer, and the filter is the `state` key of the address.

const query = (call: Call) => new URL(call.path, "http://hub.test").searchParams;

const api = (call: Call) => {
  const path = call.path.split("?")[0] ?? "";
  if (path === "/api/v1/me") return json(me("viewer"));
  if (path === "/api/v1/prs") return json({ items: query(call).get("state") === "merged" ? [prLink({ id: 2, state: "merged", pr_number: 417, closed_at: 5 })] : [prLink()], next: null });
  return json({ items: [], next: null });
};

describe("the route", () => {
  it("is the address the MCP tools send people to, and a Viewer may open it", async () => {
    const app = renderApp("/swimlanes/sitb/pull-requests", api);
    expect(await screen.findByRole("heading", { level: 1, name: "Pull requests" })).toBeInTheDocument();
    expect(await screen.findByRole("grid", { name: "Pull requests" })).toBeInTheDocument();
    expect(app.location()).toBe("/swimlanes/sitb/pull-requests");
  });

  it("changes the filter in the address, without adding a history entry", async () => {
    const app = renderApp("/swimlanes/sitb/pull-requests", api);
    await screen.findByRole("grid", { name: "Pull requests" });
    const before = app.router.history.length;
    await userEvent.click(screen.getByRole("radio", { name: "Merged" }));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitb/pull-requests?state=merged"));
    expect(await screen.findByRole("link", { name: /#417/ })).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/prs") && query(c).get("state") === "merged")).toBe(true);
    expect(app.router.history.length).toBe(before);
    await userEvent.click(screen.getByRole("radio", { name: "Open" }));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitb/pull-requests"));
    expect(await screen.findByRole("link", { name: /#418/ })).toBeInTheDocument();
  });

  it("opens on the filter the address names, and drops a key the route does not read", async () => {
    const app = renderApp("/swimlanes/sitb/pull-requests?state=merged&evil=1", api);
    await screen.findByRole("grid", { name: "Pull requests" });
    expect(screen.getByRole("radio", { name: "Merged" })).toBeChecked();
    const keys = [...new URL(app.location(), "http://hub.test").searchParams.keys()];
    expect(keys).not.toContain("evil");
  });

  it("is not found for a swimlane id that cannot be one", async () => {
    renderApp("/swimlanes/Not%20An%20Id/pull-requests", api);
    expect(await screen.findByRole("heading", { level: 1, name: "Page not found" })).toBeInTheDocument();
  });

  it("names the page in the window title", async () => {
    renderApp("/swimlanes/sitb/pull-requests", api);
    await screen.findByRole("grid", { name: "Pull requests" });
    await waitFor(() => expect(document.title).toMatch(/Pull requests/));
  });
});
