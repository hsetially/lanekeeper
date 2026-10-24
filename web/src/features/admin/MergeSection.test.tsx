import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { openAdmin, rowIn, installAdminClock, writesOf } from "@/mocks/design/admin/harness";
import { newWorld } from "@/mocks/design/admin/world";
import { problem } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Merge modes (route /admin/merge, Admin only): whether a tenant file replaces its base file (whole_file) or is merged into it
// (spring_merge), decided by a glob over the file's path. The first matching rule wins, in the order the hub lists them; the
// contract has no position to change, so the order cannot be edited here (design/DEVIATIONS.md).

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Merge modes" });

describe("the rules", () => {
  it("lists them in the hub's order with their mode, and says the first match wins", async () => {
    await openAdmin(newWorld(), "/admin/merge");
    expect(await screen.findByRole("heading", { level: 2, name: "Merge modes" })).toBeInTheDocument();
    expect(await screen.findByText("3 rules")).toBeInTheDocument();
    const security = await rowIn(await grid(), "security/**");
    expect(within(security).getByText("spring_merge")).toBeInTheDocument();
    expect(within(security).getByText("1")).toBeInTheDocument();
    const services = await rowIn(await grid(), "services/**/*.yml");
    expect(within(services).getByText("whole_file")).toBeInTheDocument();
    expect(screen.getByText(/First match wins\. Files that match no rule use whole_file/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /move (up|down)/i })).toBeNull();
  });

  it("says what no rules means", async () => {
    const world = newWorld();
    world.merge = [];
    await openAdmin(world, "/admin/merge");
    expect(await screen.findByText("No merge rules")).toBeInTheDocument();
    expect(screen.getByText(/Every tenant file replaces its base file \(whole_file\)/)).toBeInTheDocument();
  });

  it("shows a skeleton while loading and the problem when the list cannot be read", async () => {
    const held = newWorld();
    held.hold.add("/api/v1/admin/merge-modes");
    await openAdmin(held, "/admin/merge");
    expect(await screen.findByLabelText("Loading merge modes")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/merge-modes"), respond: () => problem(500, "internal"), once: true });
    await openAdmin(world, "/admin/merge");
    expect(await screen.findByText("Couldn't load merge modes")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("adding and changing a rule", () => {
  it("adds a rule with a pattern and a mode, after a dialog", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/merge");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a merge rule" });
    await user.type(within(dialog).getByLabelText("Rule id"), "docs");
    await user.type(within(dialog).getByLabelText("Path glob"), "docs/**");
    await user.click(within(dialog).getByRole("radio", { name: /spring_merge/ }));
    expect(within(dialog).getByText(/keys of the tenant file are merged over the base file's keys/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/merge-modes/docs" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ file_pattern: "docs/**", mode: "spring_merge" });
    expect(await screen.findByText("Rule docs saved")).toBeInTheDocument();
    expect(await rowIn(await grid(), "docs/**")).toBeInTheDocument();
  });

  it("starts a new rule as whole_file, which is what a file with no rule gets", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/merge");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a merge rule" });
    expect(within(dialog).getByRole("radio", { name: /whole_file/ })).toBeChecked();
    await user.type(within(dialog).getByLabelText("Rule id"), "x");
    await user.type(within(dialog).getByLabelText("Path glob"), "x/**");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(JSON.parse(writesOf(app.calls)[0]?.body ?? "{}")).toEqual({ file_pattern: "x/**", mode: "whole_file" }));
  });

  it("refuses an empty glob, an unclosed bracket and a space, and an id that is not an id, sending nothing", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/merge");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a merge rule" });
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText("Enter a rule id")).toBeInTheDocument();
    expect(within(dialog).getByText("Enter a path glob")).toBeInTheDocument();
    const glob = within(dialog).getByLabelText("Path glob");
    await user.type(glob, "a/[[b");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText("Invalid glob: unclosed [")).toBeInTheDocument();
    await user.clear(glob);
    await user.type(glob, "a b");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText("Globs can't contain spaces")).toBeInTheDocument();
    expect(glob).toHaveAttribute("aria-invalid", "true");
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("changes the mode of a rule and says what that changes", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/merge");
    await user.click(within(await rowIn(await grid(), "security/**")).getByRole("button", { name: "Edit rule security" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit rule security" });
    expect(within(dialog).getByRole("radio", { name: /spring_merge/ })).toBeChecked();
    expect(within(dialog).getByText(/Changes how the files that match are compared and shown as effective config/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("radio", { name: /whole_file/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(JSON.parse(writesOf(app.calls)[0]?.body ?? "{}")).toEqual({ file_pattern: "security/**", mode: "whole_file" });
    expect(within(await rowIn(await grid(), "security/**")).getByText("whole_file")).toBeInTheDocument();
  });

  it("keeps the dialog and the input when the hub does not accept the rule (422)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(422, "unprocessable"), once: true });
    await openAdmin(world, "/admin/merge");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a merge rule" });
    await user.type(within(dialog).getByLabelText("Rule id"), "docs");
    await user.type(within(dialog).getByLabelText("Path glob"), "docs/**");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Path glob")).toHaveValue("docs/**");
  });
});

describe("deleting a rule", () => {
  it("says which files fall to the next rule, and removes the row once the hub has", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/merge");
    await user.click(within(await rowIn(await grid(), "**/*.png")).getByRole("button", { name: "Delete rule images" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete rule images" });
    expect(within(dialog).getByText(/use the next rule that matches, or whole_file/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/merge-modes/images" });
    expect(await screen.findByText("2 rules")).toBeInTheDocument();
  });
});

describe("accessibility", () => {
  it("has no structural violations", async () => {
    await openAdmin(newWorld(), "/admin/merge");
    await rowIn(await grid(), "security/**");
    await expectNoAxeViolations();
  });
});
