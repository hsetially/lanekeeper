import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import { PICKUP_ORDER } from "./model";
import { PickupChip } from "./PickupChip";

// The chip of one pickup state (D85): an icon, the words and a colour, never a colour alone; the time a client cache expires by,
// when the hub gave one; and a tooltip that says in plain words when the change takes effect.

const NOON = Date.UTC(2026, 9, 17, 14, 0, 0);
const MIN = 60_000;
const chipOf = (container: HTMLElement): HTMLElement => {
  const chip = container.querySelector<HTMLElement>("[data-slot='pickup-chip']");
  if (chip === null) throw new Error("no chip");
  return chip;
};

describe("the chip of a state", () => {
  it.each<[PickupState, string, string]>([
    ["live", "Live", "sync"],
    ["needs_notify_or_restart", "Needs notify or restart", "restart"],
    ["needs_config_server_restart", "Needs config-server restart", "restart"],
  ])("%s is an icon, the words %s and the %s colours", (state, words, tokens) => {
    const { container } = render(<PickupChip state={state} />);
    const chip = chipOf(container);
    expect(chip.querySelector("svg")).not.toBeNull();
    expect(within(chip).getByText(words)).toBeInTheDocument();
    const badge = chip.querySelector("[data-slot='status-badge']");
    expect(badge?.className).toContain(`text-status-${tokens}-fg`);
    expect(badge?.className).toContain(`bg-status-${tokens}-bg`);
    expect(chip).toHaveAttribute("data-pickup", state);
  });

  it("is 'Live by 14:32 UTC' with '(client cache)' beside it for a resource file that waits on the cache", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={Date.UTC(2026, 9, 17, 14, 32)} now={NOON} />);
    const chip = chipOf(container);
    expect(within(chip).getByText("Live by 14:32 UTC")).toBeInTheDocument();
    expect(within(chip).getByText("(client cache)")).toBeInTheDocument();
    expect(chip.querySelector("[data-slot='status-badge']")).toHaveAttribute("data-status", "live-by");
  });

  it("does not invent a time when the hub gave none: 'Live within client cache'", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={null} now={NOON} />);
    const chip = chipOf(container);
    expect(within(chip).getByText("Live within client cache")).toBeInTheDocument();
    expect(chip.textContent).not.toMatch(/\d\d:\d\d/);
    expect(within(chip).queryByText("(client cache)")).toBeNull();
  });

  it.each([Number.NaN, -1, Number.POSITIVE_INFINITY, "soon" as unknown as number])("does not invent a time for a live_by of %j", (bad) => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={bad} now={NOON} />);
    expect(within(chipOf(container)).getByText("Live within client cache")).toBeInTheDocument();
    expect(chipOf(container).textContent).not.toMatch(/\d\d:\d\d/);
  });

  it("says the time has passed when it has, instead of presenting it as still to come", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={NOON - 5 * MIN} now={NOON} />);
    const chip = chipOf(container);
    expect(within(chip).getByText("Live by 13:55 UTC")).toBeInTheDocument();
    expect(within(chip).getByText("(time passed)")).toBeInTheDocument();
    expect(within(chip).queryByText("(client cache)")).toBeNull();
  });

  it("draws its numbers in tabular figures, so that a column of times lines up", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={Date.UTC(2026, 9, 17, 14, 32)} now={NOON} />);
    expect(chipOf(container).querySelector("[data-slot='status-badge']")?.className).toMatch(/(^|\s)tabular-nums(\s|$)/);
  });

  it("draws nothing for a state the hub did not state (null): a file's pickup state is null until the hub fills it, and null is not 'Live'", () => {
    const { container, rerender } = render(<PickupChip state={null} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<PickupChip state={undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("can leave Live out, for a list where only what is waiting is worth a mark", () => {
    const { container, rerender } = render(<PickupChip state="live" hideLive />);
    expect(container).toBeEmptyDOMElement();
    rerender(<PickupChip state="needs_notify_or_restart" hideLive />);
    expect(chipOf(container)).toBeInTheDocument();
    rerender(<PickupChip state="live_within_ttl" hideLive />);
    expect(chipOf(container)).toBeInTheDocument();
  });

  it("says 'Pickup unknown' for a state it does not know, and never repeats it", () => {
    const hostile = "<img src=x onerror=alert(1)>\u202e";
    const { container } = render(<PickupChip state={hostile as PickupState} />);
    const chip = chipOf(container);
    expect(within(chip).getByText("Pickup unknown")).toBeInTheDocument();
    expect(chip).toHaveAttribute("data-pickup", "unrecognised");
    expect(container.innerHTML).not.toContain("onerror");
    expect(container.textContent).not.toContain("<img");
    expect(container.querySelector("img")).toBeNull();
  });

  it("never says 'pending restart' (D85), in any state, in either size", () => {
    for (const state of PICKUP_ORDER) {
      for (const density of ["full", "compact"] as const) {
        const { container, unmount } = render(<PickupChip state={state} density={density} count={2} liveBy={NOON + MIN} now={NOON} />);
        expect(container.textContent, `${state} ${density}`).not.toMatch(/pending restart/i);
        expect(container.innerHTML, `${state} ${density}`).not.toMatch(/pending restart/i);
        unmount();
      }
    }
  });
});

describe("the full chip, to a screen reader", () => {
  it("reads the state and then when it takes effect, without a hover", () => {
    const { container } = render(<PickupChip state="needs_notify_or_restart" subject="service" />);
    const chip = chipOf(container);
    const spoken = chip.querySelector(".sr-only");
    expect(spoken?.textContent).toBe("A property file changed. This service takes it after it is notified or restarted.");
  });

  it("adds the lines it is given (the cache lifetime, the notifications flag) as their own sentences", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={Date.UTC(2026, 9, 17, 14, 32)} now={NOON} details={["Client cache lifetime: 5 min", "Notifications: on (as reported by the hub)"]} />);
    expect(chipOf(container).querySelector(".sr-only")?.textContent).toBe(
      "A resource file changed. This service reads it again when its client cache expires, by 2026-10-17 14:32:00 UTC. Client cache lifetime: 5 min. Notifications: on (as reported by the hub).",
    );
  });
});

describe("the compact chip, for a row with no room", () => {
  it("is an icon and a count, named for a screen reader as '2 Needs notify or restart'", () => {
    render(<PickupChip state="needs_notify_or_restart" density="compact" count={2} />);
    const chip = screen.getByRole("img", { name: "2 Needs notify or restart" });
    expect(chip.querySelector("svg")).not.toBeNull();
    expect(within(chip).getByText("2")).toBeInTheDocument();
  });

  it("keeps the words out of sight, not out of the page", () => {
    render(<PickupChip state="needs_config_server_restart" density="compact" count={1} />);
    const words = screen.getByText("Needs config-server restart");
    // The label is the badge's last span; the badge says so for that span (an arbitrary variant), so the words stay in the page.
    expect(words.closest("[data-slot='status-badge']")?.className).toContain("[&>span:last-child]:sr-only");
  });

  it("is an icon alone, still named, when there is no count (a file in a tree)", () => {
    render(<PickupChip state="needs_notify_or_restart" density="compact" subject="file" />);
    const chip = screen.getByRole("img", { name: "Needs notify or restart" });
    expect(chip.querySelector("svg")).not.toBeNull();
    expect(chip.textContent).toBe("Needs notify or restart");
  });

  it("has no native title: the tooltip is the one place the words are, so they are not drawn twice", () => {
    const { container } = render(<PickupChip state="needs_notify_or_restart" density="compact" count={2} />);
    expect(container.querySelector("[title]")).toBeNull();
  });
});

describe("the tooltip: when the change takes effect, in plain words", () => {
  const hover = async (el: HTMLElement) => {
    const user = userEvent.setup();
    await user.hover(el);
    return screen.findByRole("tooltip");
  };

  it.each<[PickupState, RegExp]>([
    ["live", /already has the current config/],
    ["live_within_ttl", /client cache expires/],
    ["needs_notify_or_restart", /after it is notified or restarted/],
    ["needs_config_server_restart", /until the config-server restarts/],
  ])("says what %s means", async (state, expected) => {
    const { container } = render(<PickupChip state={state} />);
    const tip = await hover(chipOf(container));
    expect(tip).toHaveTextContent(expected);
    expect(tip.textContent).not.toMatch(/pending restart/i);
  });

  it("gives the exact time, with the date and the seconds, for a live-by", async () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={Date.UTC(2026, 9, 17, 14, 32, 40)} now={NOON} />);
    const tip = await hover(chipOf(container));
    expect(tip).toHaveTextContent("by 2026-10-17 14:32:40 UTC");
  });

  it("says a sentence once even when a caller gives it twice", async () => {
    const { container } = render(<PickupChip state="needs_notify_or_restart" details={["A property file changed. This service takes it after it is notified or restarted."]} />);
    const tip = await hover(chipOf(container));
    expect(tip.textContent?.match(/A property file changed/g)).toHaveLength(1);
    expect(container.querySelector(".sr-only")?.textContent?.match(/A property file changed/g)).toHaveLength(1);
  });

  it("shows the lines it was given, each on its own line", async () => {
    const { container } = render(<PickupChip state="needs_notify_or_restart" details={["Notifications: off (as reported by the hub)"]} />);
    const tip = await hover(chipOf(container));
    expect(tip).toHaveTextContent("Notifications: off (as reported by the hub)");
  });

  it("opens on the compact chip too, with the words of a summary", async () => {
    render(<PickupChip state="needs_notify_or_restart" density="compact" subject="count" count={3} />);
    const tip = await hover(screen.getByRole("img", { name: "3 Needs notify or restart" }));
    expect(tip).toHaveTextContent("3 services need a notify or a restart to take a changed property file.");
  });

  it("lets a caller say what it counted instead (a folder says how many of the files it has loaded)", async () => {
    const { container } = render(<PickupChip state="needs_notify_or_restart" subject="file" words="2 of the 14 files loaded in this folder need a notify or a restart." />);
    const tip = await hover(chipOf(container));
    expect(tip).toHaveTextContent("2 of the 14 files loaded in this folder need a notify or a restart.");
    expect(tip.textContent).not.toContain("A property file:");
  });
});
