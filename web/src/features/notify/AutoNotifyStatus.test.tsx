import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Role } from "@/api/generated/model";
import { expectNoAxeViolations } from "@/test/axe";
import { AutoNotifyStatus } from "./AutoNotifyStatus";
import { laneDetail, renderNotify } from "./test-utils";

// Whether a swimlane notifies the config-server by itself after a write. Everyone who can see the swimlane sees the answer (it is a
// field of the swimlane's detail); only an Admin can change it, and that is the Admin page's switch: this is not a second one.

const show = (autoNotify: boolean, role: Role = "operator") => renderNotify(<AutoNotifyStatus lane={laneDetail({ auto_notify: autoNotify })} />, { role });

describe("AutoNotifyStatus", () => {
  it("says it is on, in words, with an icon, and what that means", async () => {
    show(true);
    const status = await screen.findByRole("group", { name: "Auto-notify" });
    expect(within(status).getByText("Auto-notify on")).toBeInTheDocument();
    expect(status.querySelector("svg")).not.toBeNull();
    expect(within(status).getByText(/after each write that is applied, the agent tells the config-server which paths changed/i)).toBeInTheDocument();
  });

  it("says it is off, in words, and that nobody is notified automatically", async () => {
    show(false);
    const status = await screen.findByRole("group", { name: "Auto-notify" });
    expect(within(status).getByText("Auto-notify off")).toBeInTheDocument();
    expect(within(status).getByText(/nobody is notified automatically/i)).toBeInTheDocument();
  });

  it("tells the on state from the off state by more than colour: the label and the icon differ", async () => {
    const { unmount } = show(true);
    const on = (await screen.findByText("Auto-notify on")).closest("[data-slot=badge]")?.innerHTML ?? "";
    unmount();
    show(false);
    const off = (await screen.findByText("Auto-notify off")).closest("[data-slot=badge]")?.innerHTML ?? "";
    expect(on).not.toEqual("");
    expect(on.replace("Auto-notify on", "")).not.toEqual(off.replace("Auto-notify off", ""));
  });

  it("offers an Admin the setting, as a link to the Admin page, and no switch of its own", async () => {
    show(true, "admin");
    const status = await screen.findByRole("group", { name: "Auto-notify" });
    expect(within(status).getByRole("link", { name: "Change in Admin" })).toHaveAttribute("href", "/admin/swimlanes");
    expect(within(status).queryByRole("switch")).toBeNull();
    expect(within(status).queryByRole("checkbox")).toBeNull();
  });

  it.each<Role>(["viewer", "editor", "operator"])("tells a %s that an Admin sets it, and offers no link", async (role) => {
    show(true, role);
    const status = await screen.findByRole("group", { name: "Auto-notify" });
    expect(within(status).getByText("Only an Admin can change this.")).toBeInTheDocument();
    expect(within(status).queryByRole("link")).toBeNull();
  });

  it("asks the hub for nothing: the answer is in the swimlane it was given", async () => {
    const { calls } = show(true, "admin");
    await screen.findByRole("group", { name: "Auto-notify" });
    expect(calls).toEqual([]);
  });

  it("has no axe violations", async () => {
    show(false, "admin");
    await screen.findByRole("group", { name: "Auto-notify" });
    await expectNoAxeViolations();
  });
});
