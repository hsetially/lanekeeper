import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { openAdmin, rowIn, installAdminClock } from "@/mocks/design/admin/harness";
import { newWorld } from "@/mocks/design/admin/world";

// The page of /admin/:section: it chooses the section, inside the Admin area. Four sections (sentinels, OS Login mapping, severity
// rules, retention) are built by task T5 and are listed in the navigation; until T5 adds them their page says so, and still has the
// area around it, so a link to one is never a blank or a 404.

installAdminClock();

describe("the sections of another task", () => {
  it.each([
    ["sentinels", "Sentinels"],
    ["oslogin", "OS Login user mapping"],
    ["severity", "Severity rules"],
    ["retention", "Retention"],
  ])("%s opens its page in the Admin area and says it is not available yet", async (section, title) => {
    await openAdmin(newWorld(), `/admin/${section}`);
    expect(await screen.findByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    expect(screen.getByText("Not available yet")).toBeInTheDocument();
    expect(screen.getByText(`${title} is not part of this build of the Admin area yet.`)).toBeInTheDocument();
    expect(within(screen.getByRole("navigation", { name: "Admin sections" })).getByRole("link", { name: title === "OS Login user mapping" ? "OS Login users" : title })).toHaveAttribute("aria-current", "page");
  });

  it("asks the hub for nothing of that section", async () => {
    const app = await openAdmin(newWorld(), "/admin/retention");
    await screen.findByText("Not available yet");
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin/retention"))).toBe(false);
  });
});

describe("the scrolling part of the area", () => {
  it("is a named region that takes keyboard focus, because it scrolls and an empty section has nothing in it to focus (axe `scrollable-region-focusable`)", async () => {
    await openAdmin(newWorld(), "/admin/retention");
    // Named "Admin section: Retention", not "Retention": the Notifications section would share a landmark name with the toaster.
    const region = await screen.findByRole("region", { name: "Admin section: Retention" });
    expect(region).toHaveAttribute("tabindex", "0");
    // It is the section's own content, and the navigation is not inside it.
    expect(within(region).getByRole("heading", { level: 2, name: "Retention" })).toBeInTheDocument();
    expect(within(region).queryByRole("navigation", { name: "Admin sections" })).toBeNull();
  });
});

describe("moving between sections", () => {
  it("follows a link of the navigation to another section and marks it current, in the same Admin area", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/paths");
    expect(await screen.findByRole("heading", { level: 2, name: "Path mappings" })).toBeInTheDocument();
    await user.click(within(screen.getByRole("navigation", { name: "Admin sections" })).getByRole("link", { name: "Merge modes" }));
    expect(await screen.findByRole("heading", { level: 2, name: "Merge modes" })).toBeInTheDocument();
    expect(app.location()).toBe("/admin/merge");
    expect(within(screen.getByRole("navigation", { name: "Admin sections" })).getByRole("link", { name: "Merge modes" })).toHaveAttribute("aria-current", "page");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    await waitFor(async () => {
      expect(await rowIn(await screen.findByRole("grid", { name: "Merge modes" }), "security/**")).toBeInTheDocument();
    });
  });

  it("goes from a section to the users and to the swimlanes by the same navigation", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/tiers");
    const nav = () => within(screen.getByRole("navigation", { name: "Admin sections" }));
    await user.click(nav().getByRole("link", { name: /^Users and access requests/ }));
    expect(await screen.findByRole("heading", { level: 2, name: "Users and access requests" })).toBeInTheDocument();
    expect(app.location()).toBe("/admin/users");
    await user.click(nav().getByRole("link", { name: "Swimlanes and agents" }));
    expect(await screen.findByRole("heading", { level: 2, name: "Swimlanes and agents" })).toBeInTheDocument();
    expect(app.location()).toBe("/admin/swimlanes");
  });
});
