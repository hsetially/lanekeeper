import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SwimlaneSummary } from "@/api/generated/model";
import { LanePicker } from "./LanePicker";

const lane = (id: string, over: Partial<SwimlaneSummary> = {}): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: [`t-${id}`],
  agent: "connected",
  last_scan_at: 0,
  base_version: null,
  drift_counts: [],
  finding_count: 0,
  tier: "SIT",
  ...over,
});
const ALL = ["sit", "sitb", "sitc", "presita"].map((id) => lane(id));
const MANY = Array.from({ length: 45 }, (_, i) => lane(`lane-${String(i).padStart(2, "0")}`));

function open(over: Partial<Parameters<typeof LanePicker>[0]> = {}) {
  const onApply = vi.fn();
  const onOpenChange = vi.fn();
  render(<LanePicker open onOpenChange={onOpenChange} all={ALL} selected={["sitb"]} limit={40} onApply={onApply} {...over} />);
  return { onApply, onOpenChange };
}

describe("choosing swimlanes", () => {
  it("lists the swimlanes with the current choice ticked, in a dialog with a title", () => {
    open();
    const dialog = screen.getByRole("dialog", { name: "Choose swimlanes" });
    expect(within(dialog).getByRole("checkbox", { name: "sitb" })).toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "sit" })).not.toBeChecked();
    expect(within(dialog).getByText("1 / 40 selected")).toBeInTheDocument();
  });

  it("hands back the choice in the order of the list, not of the clicks", async () => {
    const user = userEvent.setup();
    const { onApply } = open({ selected: [] });
    await user.click(screen.getByRole("checkbox", { name: "sitc" }));
    await user.click(screen.getByRole("checkbox", { name: "sit" }));
    await user.click(screen.getByRole("button", { name: "Compare" }));
    expect(onApply).toHaveBeenCalledWith(["sit", "sitc"]);
  });

  it("filters by name, tier and tenant", async () => {
    const user = userEvent.setup();
    open();
    await user.type(screen.getByRole("textbox", { name: "Filter swimlanes" }), "presit");
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    await user.clear(screen.getByRole("textbox", { name: "Filter swimlanes" }));
    await user.type(screen.getByRole("textbox", { name: "Filter swimlanes" }), "t-sitc");
    expect(screen.getByRole("checkbox", { name: "sitc" })).toBeInTheDocument();
    await user.clear(screen.getByRole("textbox", { name: "Filter swimlanes" }));
    await user.type(screen.getByRole("textbox", { name: "Filter swimlanes" }), "zzz");
    expect(screen.getByText("No swimlane matches.")).toBeInTheDocument();
  });

  it("stops at the limit and says so: the rest cannot be ticked", async () => {
    const user = userEvent.setup();
    open({ all: MANY, selected: MANY.slice(0, 39).map((l) => l.id) });
    await user.click(screen.getByRole("checkbox", { name: "lane-39" }));
    expect(screen.getByText(/40 \/ 40 selected\. That is the most the grid shows/)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "lane-40" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "lane-00" })).toBeEnabled();
  });

  it("cannot compare nothing", async () => {
    const user = userEvent.setup();
    const { onApply } = open({ selected: [] });
    expect(screen.getByRole("button", { name: "Compare" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onApply).not.toHaveBeenCalled();
  });

  it("keeps a swimlane it does not know about (one from the address), at the end", async () => {
    const user = userEvent.setup();
    const { onApply } = open({ selected: ["gone", "sitb"] });
    await user.click(screen.getByRole("button", { name: "Compare" }));
    expect(onApply).toHaveBeenCalledWith(["sitb", "gone"]);
  });
});
