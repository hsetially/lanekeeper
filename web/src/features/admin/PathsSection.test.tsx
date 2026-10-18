import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { FIRST, holdUntil, openAdmin, rowIn, installAdminClock, writesOf } from "@/mocks/design/admin/harness";
import { newWorld } from "@/mocks/design/admin/world";
import { problem, renderApp } from "@/test/app";
import { worldApi } from "@/mocks/design/admin/world";
import { expectNoAxeViolations } from "@/test/axe";

// Path mappings (route /admin/paths, Admin only): where the base files and the tenant files live in their Git repositories, and for
// which swimlanes. A rule is saved or deleted after a dialog that names what that does; nothing is shown as done before the hub
// answers. The design's live preview (a path mapped both ways) is not here: mapping a path is the engine's work, and no operation
// gives it to the browser.

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Path mappings" });

describe("the rules", () => {
  it("is the path mappings section, with the rules, what they apply to, and how a tenant file is named", async () => {
    await openAdmin(newWorld(), "/admin/paths");
    expect(await screen.findByRole("heading", { level: 2, name: "Path mappings" })).toBeInTheDocument();
    expect(await screen.findByText("2 rules")).toBeInTheDocument();
    const def = await rowIn(await grid(), "default");
    expect(within(def).getByText("config")).toBeInTheDocument();
    expect(within(def).getByText("data/config")).toBeInTheDocument();
    expect(within(def).getByText("Every swimlane")).toBeInTheDocument();
    const legacy = await rowIn(await grid(), "presit-legacy");
    expect(within(legacy).getByText("presita, presitb")).toBeInTheDocument();
    expect(screen.getByText(/the tenant's name is added before the extension/i)).toBeInTheDocument();
    expect(screen.queryByText(/live preview/i)).toBeNull();
  });

  it("is an empty state with the way to add the first rule", async () => {
    const world = newWorld();
    world.paths = [];
    await openAdmin(world, "/admin/paths");
    expect(await screen.findByText("No path mappings")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add rule" })).toBeInTheDocument();
  });

  it("shows a skeleton while loading and the request id when the list cannot be read", async () => {
    const held = newWorld();
    held.hold.add("/api/v1/admin/path-mappings");
    await openAdmin(held, "/admin/paths");
    expect(await screen.findByLabelText("Loading path mappings")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id and a retry", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/path-mappings"), respond: () => problem(500, "internal"), once: true });
    await openAdmin(world, "/admin/paths");
    expect(await screen.findByText("Couldn't load path mappings")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(await rowIn(await grid(), "default")).toBeInTheDocument();
  });
});

describe("adding a rule", () => {
  it("sends the roots and the swimlanes chosen, after a dialog, and says it was saved", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await openAdmin(world, "/admin/paths");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    await user.type(within(dialog).getByLabelText("Rule id"), "legacy-docs");
    await user.type(within(dialog).getByLabelText("Base repo path"), "docs/base");
    await user.type(within(dialog).getByLabelText("Tenant repo path"), "docs/tenant");
    await user.click(within(dialog).getByRole("radio", { name: "Only some swimlanes" }));
    await user.click(await within(dialog).findByRole("checkbox", { name: "presita" }));
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put?.method).toBe("PUT");
    expect(put?.path).toBe("/api/v1/admin/path-mappings/legacy-docs");
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ base_root: "docs/base", tenant_root: "docs/tenant", swimlanes: ["presita"] });
    expect(put?.headers.get("X-CSRF-Token")).toBeTruthy();
    expect(put?.headers.get("Idempotency-Key")).toBeTruthy();
    expect(await screen.findByText("Rule legacy-docs saved")).toBeInTheDocument();
    expect(await rowIn(await grid(), "legacy-docs")).toBeInTheDocument();
    expect(screen.getByText("3 rules")).toBeInTheDocument();
  });

  it("starts with focus in the first field, and gives it back to the button that opened the dialog when it closes", async () => {
    const user = userEvent.setup();
    await openAdmin(newWorld(), "/admin/paths");
    const add = await screen.findByRole("button", { name: "Add rule" });
    await user.click(add);
    const dialog = await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    expect(within(dialog).getByLabelText("Rule id")).toHaveFocus();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(add).toHaveFocus();
  });

  it("applies to every swimlane unless told otherwise", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/paths");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    expect(within(dialog).getByRole("radio", { name: "Every swimlane" })).toBeChecked();
    await user.type(within(dialog).getByLabelText("Rule id"), "all");
    await user.type(within(dialog).getByLabelText("Base repo path"), "a");
    await user.type(within(dialog).getByLabelText("Tenant repo path"), "b");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(JSON.parse(writesOf(app.calls)[0]?.body ?? "{}")).toEqual({ base_root: "a", tenant_root: "b", swimlanes: [] }));
  });

  it("says what is wrong next to each field and sends nothing", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/paths");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    await user.type(within(dialog).getByLabelText("Rule id"), "Bad Id");
    await user.type(within(dialog).getByLabelText("Base repo path"), "/config");
    await user.type(within(dialog).getByLabelText("Tenant repo path"), "a/../b");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText(/lower case letters, digits/i)).toBeInTheDocument();
    expect(within(dialog).getByText("Repo paths are relative. Remove the leading /")).toBeInTheDocument();
    expect(within(dialog).getByText("A path can't contain .. segments")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Base repo path")).toHaveAttribute("aria-invalid", "true");
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("wants at least one swimlane when it is for some swimlanes", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/paths");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    await user.type(within(dialog).getByLabelText("Rule id"), "some");
    await user.type(within(dialog).getByLabelText("Base repo path"), "a");
    await user.type(within(dialog).getByLabelText("Tenant repo path"), "b");
    await user.click(within(dialog).getByRole("radio", { name: "Only some swimlanes" }));
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText("Choose at least one swimlane")).toBeInTheDocument();
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("keeps the dialog and everything typed when the hub does not accept the rule (422)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(422, "unprocessable"), once: true });
    await openAdmin(world, "/admin/paths");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    await user.type(within(dialog).getByLabelText("Rule id"), "x");
    await user.type(within(dialog).getByLabelText("Base repo path"), "a");
    await user.type(within(dialog).getByLabelText("Tenant repo path"), "b");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Base repo path")).toHaveValue("a");
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("changing a rule", () => {
  it("keeps the id, says it remaps files, and shows the hub's answer only after it comes", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const { gate, release } = holdUntil();
    world.overrides.push({
      match: (c) => c.method === "PUT",
      respond: async (call) => {
        await gate;
        const rule = { id: "default", ...(JSON.parse(call.body ?? "{}") as object) };
        world.paths = world.paths.map((r) => (r.id === "default" ? (rule as (typeof world.paths)[number]) : r));
        return new Response(JSON.stringify(rule), { status: 200, headers: { "content-type": "application/json" } });
      },
      once: true,
    });
    const app = await openAdmin(world, "/admin/paths");
    await user.click(within(await rowIn(await grid(), "default")).getByRole("button", { name: "Edit rule default" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit rule default" });
    expect(within(dialog).getByLabelText("Rule id")).toHaveAttribute("readonly");
    expect(within(dialog).getByText(/remaps every file this rule covers/i)).toBeInTheDocument();
    const tenant = within(dialog).getByLabelText("Tenant repo path");
    await user.clear(tenant);
    await user.type(tenant, "data/config-v2");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByRole("button", { name: "Saving…" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("data/config", { selector: "bdi" })).toBeInTheDocument();
    release();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]?.path).toBe("/api/v1/admin/path-mappings/default");
    expect(await within(await grid()).findByText("data/config-v2")).toBeInTheDocument();
  });
});

describe("deleting a rule", () => {
  it("asks first, says what happens to the files, and removes the row once the hub has", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/paths");
    await user.click(within(await rowIn(await grid(), "presit-legacy")).getByRole("button", { name: "Delete rule presit-legacy" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete rule presit-legacy" });
    expect(within(dialog).getByText(/Files this rule covers are no longer matched/i)).toBeInTheDocument();
    expect(writesOf(app.calls)).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Delete rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/path-mappings/presit-legacy" });
    expect(await screen.findByText("1 rule")).toBeInTheDocument();
  });

  it("says the rule no longer exists when someone else deleted it first (404)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "DELETE", respond: () => problem(404, "not_found"), once: true });
    await openAdmin(world, "/admin/paths");
    await user.click(within(await rowIn(await grid(), "default")).getByRole("button", { name: "Delete rule default" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete rule default" });
    await user.click(within(dialog).getByRole("button", { name: "Delete rule" }));
    expect(await within(dialog).findByText(/no longer exists/i)).toBeInTheDocument();
  });
});

describe("accessibility", () => {
  it("has no structural violations with the rules and the editor open", async () => {
    const user = userEvent.setup();
    await openAdmin(newWorld(), "/admin/paths");
    await rowIn(await grid(), "default");
    await expectNoAxeViolations();
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    await screen.findByRole("alertdialog", { name: "Add a path mapping" });
    await expectNoAxeViolations();
  });
});

describe("a Viewer", () => {
  it("gets the permission state and the hub is asked for nothing", async () => {
    const app = renderApp("/admin/paths", worldApi(newWorld("viewer")));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});
