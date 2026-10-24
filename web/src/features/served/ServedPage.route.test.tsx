import { readFileSync } from "node:fs";
import { join } from "node:path";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { routeById } from "@/routes/table";
import { servedFile } from "@/mocks/design/served/data";
import { detailOf, lane } from "@/mocks/design/swimlanes/fixtures";
import { json, me, problem, renderApp, type Call } from "@/test/app";
import { WEB } from "@/test/source-files";
import { OPERATION_ROLES } from "@/api/roles.generated";

// The page in the real app: the router, the session, the shell, and the address the MCP tool `get_served_file` sends.

// The template is read back from the hub's source, so a change to it there fails here.
const source = readFileSync(join(WEB, "..", "crates/hub-mcp/src/output/links.rs"), "utf8");
const TEMPLATE = /served: t\("([^"]+)"\)/.exec(source)?.[1] ?? "";
const fill = (values: Record<string, string>): string => TEMPLATE.replace(/\{(\w+)\}/g, (_m, name: string) => values[name] ?? `MISSING-${name}`);

const FILE = "tx-infinity-api/connection-pool.yml";
const LINK = fill({ swimlane: "sitb", application: "tx-infinity-core", tenant: "sit7", path: encodeURIComponent(FILE) });

const hub = (role: string, over: (call: Call) => Response | undefined = () => undefined) => (call: Call): Response => {
  const path = call.path.split("?")[0] ?? call.path;
  const custom = over(call);
  if (custom) return custom;
  if (path === "/api/v1/me") return json(me(role));
  if (path === "/api/v1/swimlanes") return json({ items: [lane("sitb", { tenants: ["sit7", "sit9"] })], next: null });
  if (path === "/api/v1/swimlanes/sitb") return json(detailOf(lane("sitb", { tenants: ["sit7", "sit9"] })));
  if (path === "/api/v1/swimlanes/sitb/served") return json(servedFile());
  if (path === "/api/v1/proposals" || path === "/api/v1/admin/access-requests") return json({ items: [], next: null });
  return problem(404, "not_found");
};

describe("the link the MCP tool sends", () => {
  it("is read from the hub's source", () => {
    expect(TEMPLATE).toBe("/swimlanes/{swimlane}/served?application={application}&tenant={tenant}&file={path}");
  });

  it("opens the page, and the page asks the hub for exactly what the link says", async () => {
    const app = renderApp(LINK, hub("viewer"));
    expect(await screen.findByRole("heading", { level: 1, name: "As served" })).toBeInTheDocument();
    await screen.findByRole("table", { name: /Served text/ });
    const asked = app.calls.filter((c) => c.path.startsWith("/api/v1/swimlanes/sitb/served"));
    expect(asked).toHaveLength(1);
    expect(Object.fromEntries(new URL(asked[0]?.path ?? "", "http://hub.test").searchParams)).toEqual({ application: "tx-infinity-core", tenant: "sit7", file: FILE });
    expect(app.location()).toBe(`/swimlanes/sitb/served?application=tx-infinity-core&file=${encodeURIComponent(FILE)}&tenant=sit7`);
  });

  it("is open to a Viewer, and the route asks for the role the operation asks for", async () => {
    expect(routeById("swimlane-served").access).toBe(OPERATION_ROLES.getServedFile.role);
    renderApp(LINK, hub("viewer"));
    expect(await screen.findByRole("heading", { level: 1, name: "As served" })).toBeInTheDocument();
    expect(screen.queryByText("You do not have access")).toBeNull();
  });

  it("keeps only the keys the route reads: a stray key neither reaches the page nor the request", async () => {
    const app = renderApp(`${LINK}&channel=beta&evil=1&q=2`, hub("viewer"));
    await screen.findByRole("table", { name: /Served text/ });
    const asked = app.calls.find((c) => c.path.startsWith("/api/v1/swimlanes/sitb/served"));
    const params = Object.fromEntries(new URL(asked?.path ?? "", "http://hub.test").searchParams);
    expect(params).toEqual({ application: "tx-infinity-core", tenant: "sit7", channel: "beta", file: FILE });
  });

  it("moves to the address of a new selection when the button is pressed, and the page follows it", async () => {
    const app = renderApp(LINK, hub("viewer"));
    await screen.findByRole("table", { name: /Served text/ });
    await waitFor(() => expect(screen.getByLabelText("Tenant").querySelectorAll("option")).toHaveLength(3));
    await userEvent.selectOptions(screen.getByLabelText("Tenant"), "sit9");
    await userEvent.type(screen.getByLabelText("Channel"), "beta");
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    await waitFor(() => expect(app.location()).toBe(`/swimlanes/sitb/served?application=tx-infinity-core&channel=beta&file=${encodeURIComponent(FILE)}&tenant=sit9`));
    await waitFor(() => expect(app.calls.filter((c) => c.path.startsWith("/api/v1/swimlanes/sitb/served"))).toHaveLength(2));
    const second = app.calls.filter((c) => c.path.startsWith("/api/v1/swimlanes/sitb/served"))[1];
    expect(new URL(second?.path ?? "", "http://hub.test").searchParams.get("channel")).toBe("beta");
  });

  it("shows a swimlane that does not exist as not served, not as a crash", async () => {
    renderApp(LINK, hub("viewer", (call) => (call.path.startsWith("/api/v1/swimlanes/sitb") ? problem(404, "not_found") : undefined)));
    expect(await screen.findByText("Not served for this selection")).toBeInTheDocument();
  });
});
