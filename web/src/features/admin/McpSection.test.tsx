import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mcpToken } from "@/mocks/design/admin/fixtures";
import { openAdmin, installAdminClock, writesOf } from "@/mocks/design/admin/harness";
import { newWorld } from "@/mocks/design/admin/world";
import { expectNoAxeViolations } from "@/test/axe";

// MCP personal tokens (route /admin/mcp, Admin only): the section mounts the shared PersonalTokens component, which shows the
// SIGNED-IN person's own tokens. The design lists everyone's tokens with an owner and a Revoke for an Admin, and issues a token for
// another user; the contract has only "my tokens" operations, so an Admin sees and makes their own (design/DEVIATIONS.md).

installAdminClock();

describe("the section", () => {
  it("says whose tokens these are and what a token lets an agent do, and shows the person's own", async () => {
    const world = newWorld();
    world.mcp = [mcpToken(1, "laptop")];
    await openAdmin(world, "/admin/mcp");
    expect(await screen.findByRole("heading", { level: 2, name: "MCP personal tokens" })).toBeInTheDocument();
    expect(screen.getByText(/These are your own tokens/)).toBeInTheDocument();
    expect(screen.getByText(/an Admin other than the owner approves each one/i)).toBeInTheDocument();
    expect(await screen.findByRole("list", { name: "Personal MCP tokens" })).toBeInTheDocument();
    expect(screen.getByText("laptop")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create token" })).toBeInTheDocument();
  });

  it("says personal tokens are not switched on, with no form, when the hub answers the list with 404 (MCP mode is off)", async () => {
    const world = newWorld();
    world.mcp = null;
    await openAdmin(world, "/admin/mcp");
    expect(await screen.findByText("Personal MCP tokens are not switched on")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create token" })).toBeNull();
  });

  it("makes a token for the Admin, shows it once, and keeps it out of every cache", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await openAdmin(world, "/admin/mcp");
    await user.type(await screen.findByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    expect(await screen.findByLabelText("Your new token")).toHaveValue(world.mcpSecret);
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "POST", path: "/api/v1/me/mcp-tokens" });
    const held = JSON.stringify([app.queryClient.getQueryCache().getAll().map((q) => q.state.data), app.queryClient.getMutationCache().getAll().map((m) => [m.state.variables, m.state.data])]);
    expect(held).not.toContain(world.mcpSecret);
    await user.click(screen.getByRole("button", { name: "I have stored it" }));
    await waitFor(() => expect(screen.queryByLabelText("Your new token")).toBeNull());
  });

  it("revokes a token after a dialog, and the list shows it revoked", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.mcp = [mcpToken(1, "laptop")];
    const app = await openAdmin(world, "/admin/mcp");
    await user.click(await screen.findByRole("button", { name: "Revoke laptop" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Revoke laptop" });
    await user.click(within(dialog).getByRole("button", { name: "Revoke token" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/me/mcp-tokens/1" });
    expect(await screen.findByText("Revoked")).toBeInTheDocument();
  });

  it("has no structural violations", async () => {
    const world = newWorld();
    world.mcp = [mcpToken(1, "laptop")];
    await openAdmin(world, "/admin/mcp");
    await screen.findByText("laptop");
    await expectNoAxeViolations();
  });
});

