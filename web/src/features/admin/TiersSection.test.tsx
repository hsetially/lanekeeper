import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { openAdmin, rowIn, installAdminClock, writesOf } from "@/mocks/design/admin/harness";
import { newWorld } from "@/mocks/design/admin/world";
import { problem } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Host tiers (route /admin/tiers, Admin only): host-name patterns that put a host found in a config value into a tier (dev, uat, prod),
// so a finding can say that a prod host is in a SIT file. The design's test field and its table of the hosts found in config are
// the engine's work (which host is in which file, and which tier a name falls in); no operation gives them, so they are not drawn.

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Host tiers" });

describe("the patterns", () => {
  it("lists them with their tier, and says what a pattern is", async () => {
    await openAdmin(newWorld(), "/admin/tiers");
    expect(await screen.findByRole("heading", { level: 2, name: "Host tiers" })).toBeInTheDocument();
    expect(await screen.findByText("3 patterns")).toBeInTheDocument();
    const dev = await rowIn(await grid(), "*-dev.example.net");
    // The tier and the rule's id are both "dev" in this data; the tier is the second cell.
    expect(within(dev).getAllByRole("gridcell")[1]).toHaveTextContent("dev");
    expect(screen.getByText(/\* matches any characters/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Hostname")).toBeNull();
  });

  it("warns that every host is unclassified when there is no pattern", async () => {
    const world = newWorld();
    world.tiers = [];
    await openAdmin(world, "/admin/tiers");
    expect(await screen.findByText("No host patterns")).toBeInTheDocument();
    expect(screen.getByText(/Every hostname is unclassified, so findings can't flag a prod host in a SIT file/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add pattern" })).toBeInTheDocument();
  });

  it("shows a skeleton while loading and the problem when the list cannot be read", async () => {
    const held = newWorld();
    held.hold.add("/api/v1/admin/host-tiers");
    await openAdmin(held, "/admin/tiers");
    expect(await screen.findByLabelText("Loading host tiers")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/host-tiers"), respond: () => problem(500, "internal"), once: true });
    await openAdmin(world, "/admin/tiers");
    expect(await screen.findByText("Couldn't load host tiers")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("adding, changing and deleting", () => {
  it("adds a pattern with its tier after a dialog that says what it changes", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/tiers");
    await user.click(await screen.findByRole("button", { name: "Add pattern" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a host pattern" });
    expect(within(dialog).getByText(/Hosts that match are put in this tier by the next scan/i)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Rule id"), "uat-pre");
    await user.type(within(dialog).getByLabelText("Host pattern"), "*-pre.example.net");
    await user.type(within(dialog).getByLabelText("Tier"), "uat");
    await user.click(within(dialog).getByRole("button", { name: "Save pattern" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/host-tiers/uat-pre" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ host_pattern: "*-pre.example.net", tier: "uat" });
    expect(await screen.findByText("Pattern uat-pre saved")).toBeInTheDocument();
    expect(await rowIn(await grid(), "*-pre.example.net")).toBeInTheDocument();
  });

  it("refuses an address with a scheme, an empty tier and an id that is not an id, sending nothing", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/tiers");
    await user.click(await screen.findByRole("button", { name: "Add pattern" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a host pattern" });
    await user.type(within(dialog).getByLabelText("Rule id"), "Bad Id");
    await user.type(within(dialog).getByLabelText("Host pattern"), "https://a.example.net");
    await user.click(within(dialog).getByRole("button", { name: "Save pattern" }));
    expect(await within(dialog).findByText(/lower case letters, digits/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/a host name, not an address with a scheme or a path/i)).toBeInTheDocument();
    expect(within(dialog).getByText("Enter a tier")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Host pattern")).toHaveAttribute("aria-invalid", "true");
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("changes the tier of a pattern", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/tiers");
    await user.click(within(await rowIn(await grid(), "*-dev.example.net")).getByRole("button", { name: "Edit pattern dev" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit pattern dev" });
    const tier = within(dialog).getByLabelText("Tier");
    await user.clear(tier);
    await user.type(tier, "uat");
    await user.click(within(dialog).getByRole("button", { name: "Save pattern" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(JSON.parse(writesOf(app.calls)[0]?.body ?? "{}")).toEqual({ host_pattern: "*-dev.example.net", tier: "uat" });
  });

  it("keeps the dialog when the hub does not accept the pattern (422)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(422, "unprocessable"), once: true });
    await openAdmin(world, "/admin/tiers");
    await user.click(await screen.findByRole("button", { name: "Add pattern" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a host pattern" });
    await user.type(within(dialog).getByLabelText("Rule id"), "x");
    await user.type(within(dialog).getByLabelText("Host pattern"), "*.example.net");
    await user.type(within(dialog).getByLabelText("Tier"), "dev");
    await user.click(within(dialog).getByRole("button", { name: "Save pattern" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Host pattern")).toHaveValue("*.example.net");
  });

  it("deletes a pattern after saying that its hosts become unclassified", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/tiers");
    await user.click(within(await rowIn(await grid(), "*.prod.example.net")).getByRole("button", { name: "Delete pattern prod" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete pattern prod" });
    expect(within(dialog).getByText(/Hosts this pattern matched are unclassified unless another pattern matches them/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete pattern" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/host-tiers/prod" });
    expect(await screen.findByText("2 patterns")).toBeInTheDocument();
  });
});

describe("accessibility", () => {
  it("has no structural violations", async () => {
    await openAdmin(newWorld(), "/admin/tiers");
    await rowIn(await grid(), "*-dev.example.net");
    await expectNoAxeViolations();
  });
});
