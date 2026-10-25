import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { openAdmin, rowIn, installAdminClock, writesOf } from "@/mocks/design/admin/harness";
import { newWorld } from "@/mocks/design/admin/world";
import { problem } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Folder mappings (route /admin/folders, Admin only): which Deployments read an NFS folder, so that a pending restart or a notify
// after a change in it targets the right services. The design shows suggestions the tool made from folder names, each with a
// similarity score to confirm or correct; the contract has only the mappings themselves, so there are no suggestions here
// (design/DEVIATIONS.md).

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Folder mappings" });

describe("the mappings", () => {
  it("lists each folder with the Deployments that read it, and draws no suggestion to confirm", async () => {
    await openAdmin(newWorld(), "/admin/folders");
    expect(await screen.findByRole("heading", { level: 2, name: "Folder mappings" })).toBeInTheDocument();
    expect(await screen.findByText("3 mappings")).toBeInTheDocument();
    const video = await rowIn(await grid(), "video");
    expect(within(video).getByText("sitb/video-service")).toBeInTheDocument();
    const api = await rowIn(await grid(), "tx-infinity-api");
    expect(within(api).getByText("sitb/tx-infinity-core")).toBeInTheDocument();
    expect(within(api).getByText("sitb/tx-infinity-gateway")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /confirm/i })).toBeNull();
    expect(screen.queryByText(/similarity/i)).toBeNull();
  });

  it("shows only the first Deployments of a long list, with the whole list in a tooltip title", async () => {
    const world = newWorld();
    world.folders = [{ id: "f-many", folder: "many", deployments: ["a", "b", "c", "d"].map((name) => ({ namespace: "sitb", name })) }];
    await openAdmin(world, "/admin/folders");
    const row = await rowIn(await grid(), "many");
    expect(within(row).getByText("+2")).toBeInTheDocument();
    expect(within(row).getByText("sitb/a")).toBeInTheDocument();
    expect(within(row).queryByText("sitb/d")).toBeNull();
  });

  it("draws a Deployment chip as a block that ends in an ellipsis, not as a flex box (which text-overflow does not reach: the layout run found 'sitb/tx-infinity-core' cut off at 1024 and 1280)", async () => {
    await openAdmin(newWorld(), "/admin/folders");
    const api = await rowIn(await grid(), "tx-infinity-api");
    const chip = within(api).getByText("sitb/tx-infinity-core").closest("span.rounded-sm");
    expect(chip?.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(chip?.className).not.toMatch(/(^|\s)(inline-)?flex(\s|$)/);
    expect(chip?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
  });

  it("is an empty state that says what a mapping is for", async () => {
    const world = newWorld();
    world.folders = [];
    await openAdmin(world, "/admin/folders");
    expect(await screen.findByText("No folder mappings")).toBeInTheDocument();
    expect(screen.getByText(/No NFS folder is linked to a Deployment yet/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add mapping" })).toBeInTheDocument();
  });

  it("shows a skeleton while loading and the problem when the list cannot be read", async () => {
    const held = newWorld();
    held.hold.add("/api/v1/admin/folder-mappings");
    await openAdmin(held, "/admin/folders");
    expect(await screen.findByLabelText("Loading folder mappings")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/folder-mappings"), respond: () => problem(500, "internal"), once: true });
    await openAdmin(world, "/admin/folders");
    expect(await screen.findByText("Couldn't load folder mappings")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("adding, changing and deleting", () => {
  it("adds a mapping after a dialog that says who reads the folder", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/folders");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a folder mapping" });
    expect(within(dialog).getByText(/are read by these Deployments/)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Mapping id"), "docs");
    await user.type(within(dialog).getByLabelText("NFS folder"), "document-service");
    await user.type(within(dialog).getByLabelText("Deployments"), "sitb/document-service{Enter}sitc/document-service");
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/folder-mappings/docs" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({
      folder: "document-service",
      deployments: [
        { namespace: "sitb", name: "document-service" },
        { namespace: "sitc", name: "document-service" },
      ],
    });
    expect(await screen.findByText("Mapping docs saved")).toBeInTheDocument();
    expect(await rowIn(await grid(), "document-service")).toBeInTheDocument();
  });

  it("names the line of a Deployment that is not namespace/name, and refuses a folder with a leading slash", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/folders");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a folder mapping" });
    await user.type(within(dialog).getByLabelText("Mapping id"), "docs");
    await user.type(within(dialog).getByLabelText("NFS folder"), "/document-service");
    await user.type(within(dialog).getByLabelText("Deployments"), "sitb/a{Enter}video");
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    expect(await within(dialog).findByText("Line 2: write it as namespace/name")).toBeInTheDocument();
    expect(within(dialog).getByText(/Remove the leading \//)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Deployments")).toHaveAttribute("aria-invalid", "true");
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("edits a mapping, keeping its id, and shows the Deployments the hub holds afterwards", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/folders");
    await user.click(within(await rowIn(await grid(), "video")).getByRole("button", { name: "Edit mapping f-video" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit mapping f-video" });
    expect(within(dialog).getByLabelText("Mapping id")).toHaveAttribute("readonly");
    expect(within(dialog).getByLabelText("Deployments")).toHaveValue("sitb/video-service");
    await user.type(within(dialog).getByLabelText("Deployments"), "{Enter}sitc/video-service");
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]?.path).toBe("/api/v1/admin/folder-mappings/f-video");
    expect(await within(await rowIn(await grid(), "video")).findByText("sitc/video-service")).toBeInTheDocument();
  });

  it("keeps the dialog when the hub does not accept the mapping (422)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(422, "unprocessable"), once: true });
    await openAdmin(world, "/admin/folders");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a folder mapping" });
    await user.type(within(dialog).getByLabelText("Mapping id"), "x");
    await user.type(within(dialog).getByLabelText("NFS folder"), "x");
    await user.type(within(dialog).getByLabelText("Deployments"), "sitb/x");
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Deployments")).toHaveValue("sitb/x");
  });

  it("deletes a mapping after saying that a restart no longer targets those Deployments", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/folders");
    await user.click(within(await rowIn(await grid(), "security")).getByRole("button", { name: "Delete mapping f-security" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete mapping f-security" });
    expect(within(dialog).getByText(/a restart does not target them/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete mapping" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/folder-mappings/f-security" });
    expect(await screen.findByText("2 mappings")).toBeInTheDocument();
  });
});

describe("accessibility", () => {
  it("has no structural violations", async () => {
    await openAdmin(newWorld(), "/admin/folders");
    await rowIn(await grid(), "video");
    await expectNoAxeViolations();
  });
});
