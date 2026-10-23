import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import { PickupCounts } from "./PickupCounts";

// The Pickup column of the swimlane list and the chips under a swimlane's name: how many services are in each state that is not
// live. Each state has its own chip with its own count. (The first version drew one chip, in the colour of the most urgent state,
// with the TOTAL beside it: one service that needs a config-server restart and two that need a notify read "3 Needs
// config-server restart", which is not what the hub said.)

type Pairs = ReadonlyArray<readonly [PickupState, number]>;
const MIXED: Pairs = [
  ["needs_notify_or_restart", 2],
  ["needs_config_server_restart", 1],
  ["live_within_ttl", 4],
];

describe("one chip for each state, with its own count", () => {
  it("never adds the counts of two states under the name of one", () => {
    render(<PickupCounts counts={MIXED} density="compact" />);
    expect(screen.getByRole("img", { name: "1 Needs config-server restart" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "2 Needs notify or restart" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /^3 / })).toBeNull();
  });

  it("puts the more urgent state first", () => {
    const { container } = render(<PickupCounts counts={MIXED} density="compact" />);
    const names = [...container.querySelectorAll("[role='img']")].map((el) => el.getAttribute("aria-label"));
    expect(names).toEqual(["1 Needs config-server restart", "2 Needs notify or restart"]);
  });

  it("draws the full words and the count in the full size", () => {
    const { container } = render(<PickupCounts counts={MIXED} />);
    const chips = [...container.querySelectorAll("[data-slot='pickup-chip']")];
    expect(chips).toHaveLength(2);
    expect(chips[0]?.textContent).toContain("1");
    expect(chips[0]?.textContent).toContain("Needs config-server restart");
    expect(chips[1]?.textContent).toContain("2");
    expect(chips[1]?.textContent).toContain("Needs notify or restart");
  });

  it("leaves out the services that pick the change up by themselves, unless asked for them", () => {
    const { rerender } = render(<PickupCounts counts={MIXED} density="compact" />);
    expect(screen.queryByRole("img", { name: /Live/ })).toBeNull();
    rerender(<PickupCounts counts={MIXED} density="compact" include="all" />);
    expect(screen.getByRole("img", { name: "4 Live within client cache" })).toBeInTheDocument();
  });

  it("names, in the tooltip, how many services and what they wait for", async () => {
    const user = userEvent.setup();
    render(<PickupCounts counts={[["needs_notify_or_restart", 3]]} density="compact" />);
    await user.hover(screen.getByRole("img", { name: "3 Needs notify or restart" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("3 services need a notify or a restart to take a changed property file.");
  });

  it("says '1 service needs' for one", async () => {
    const user = userEvent.setup();
    render(<PickupCounts counts={[["needs_config_server_restart", 1]]} density="compact" />);
    await user.hover(screen.getByRole("img", { name: "1 Needs config-server restart" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("1 service waits for the config-server to restart");
  });

  it("ignores a count that is not a whole positive number, and a state it does not know", () => {
    const { container } = render(
      <PickupCounts
        counts={[
          ["needs_notify_or_restart", -1],
          ["needs_config_server_restart", Number.NaN],
          ["pending_restart" as PickupState, 5],
        ]}
        density="compact"
      />,
    );
    expect(container.querySelector("[data-slot='pickup-chip']")).toBeNull();
  });
});

describe("when nothing is waiting", () => {
  it("draws nothing, for a header that has no room for 'none'", () => {
    const { container } = render(<PickupCounts counts={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("draws a muted 0 with the words, for a column that must not be empty", () => {
    const { container } = render(<PickupCounts counts={undefined} density="compact" whenNone="zero" />);
    const zero = container.querySelector("[data-slot='pickup-counts']");
    expect(zero?.textContent).toBe("0");
    expect(zero).toHaveAttribute("title", "Every service has picked up its config");
    expect(zero?.className).toMatch(/tabular-nums/);
  });

  it("says what is left in the 0 when only services that pick the change up by themselves remain", () => {
    const { container } = render(<PickupCounts counts={[["live_within_ttl", 4]]} density="compact" whenNone="zero" />);
    expect(container.querySelector("[data-slot='pickup-counts']")).toHaveAttribute("title", "4 live within their cache time");
  });
});
