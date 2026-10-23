import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import { NotificationsNote } from "./NotificationsNote";
import { PickupChip } from "./PickupChip";
import { PickupCounts } from "./PickupCounts";

// Hostile data (S8, S12). The only things the hub gives the chips are a state, a time, a number and a flag, so a hostile hub or
// a hub newer than this app can put in them: a state that is not one of the four (and may be markup), a time that is a string, a
// count that is not a number, a flag that is not a boolean. None of it may become text, markup, an attribute or a link, and none
// may make a chip say something the hub did not (a time to come, a state it did not give, a flag it did not set).

const MARKUP = ["<img src=x onerror=alert(1)>", "<script>alert(1)</script>", "\"><svg onload=alert(1)>", "javascript:alert(1)", "{{constructor.constructor('alert(1)')()}}", "\u202e\u2066\u200b\u0000\ufeff"];
const NOON = Date.UTC(2026, 9, 17, 14, 0, 0);

/** Nothing under `root` may be able to run, load or navigate. */
function inert(root: ParentNode): void {
  expect(root.querySelector("img, script, svg[onload], iframe, object, embed, a[href], form, input, button")).toBeNull();
  for (const el of root.querySelectorAll("*")) for (const attr of Array.from(el.attributes)) expect(attr.name.startsWith("on"), `${attr.name} on <${el.tagName.toLowerCase()}>`).toBe(false);
}

describe.each(MARKUP)("a state of %j", (hostile) => {
  it("is 'Pickup unknown' in the full chip: the value is never drawn, named or attached", () => {
    const { container } = render(<PickupChip state={hostile as PickupState} />);
    expect(container.textContent).toContain("Pickup unknown");
    expect(container.textContent).not.toContain(hostile);
    expect(container.innerHTML).not.toContain(hostile.replace(/[<>"&]/g, ""));
    inert(container);
  });

  it("is 'Pickup unknown' in the compact chip, with the same name for a screen reader", () => {
    render(<PickupChip state={hostile as PickupState} density="compact" count={2} />);
    expect(screen.getByRole("img", { name: "2 Pickup unknown" })).toBeInTheDocument();
  });

  it("says in the tooltip that the app does not know it, without repeating it", async () => {
    const user = userEvent.setup();
    const { container } = render(<PickupChip state={hostile as PickupState} />);
    await user.hover(container.querySelector("[data-slot='pickup-chip']") as HTMLElement);
    const tip = await screen.findByRole("tooltip");
    expect(tip).toHaveTextContent("this version of the app does not know");
    expect(tip.textContent).not.toContain(hostile);
  });

  it("is not counted as a state of its own in a summary", () => {
    const { container } = render(<PickupCounts counts={[[hostile as PickupState, 3]]} density="compact" />);
    expect(container.querySelector("[data-slot='pickup-chip']")).toBeNull();
  });
});

describe("a time that is not a time", () => {
  it.each<[string, unknown]>([
    ["a string", "2026-10-17T14:32:00Z"],
    ["markup", "<img src=x onerror=alert(1)>"],
    ["not a number", Number.NaN],
    ["infinity", Number.POSITIVE_INFINITY],
    ["before 1970", -86_400_000],
    ["after 9999", 253_402_300_800_000],
    ["an object", { valueOf: () => NOON }],
    ["a boolean", true],
  ])("is no time at all for %s: no clock is drawn and none is made up", (_name, bad) => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={bad as number} now={NOON} />);
    expect(container.textContent).toContain("Live within client cache");
    expect(container.textContent).not.toMatch(/\d\d:\d\d/);
    expect(container.textContent).not.toMatch(/\d{4}-\d\d-\d\d/);
    inert(container);
  });

  it("is not drawn as the time of the page's clock either", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={Number.NaN} now={NOON} />);
    expect(container.textContent).not.toContain("14:00");
  });

  it("has no clock to judge by when 'now' is not a number, and still shows the time it was given", () => {
    const { container } = render(<PickupChip state="live_within_ttl" liveBy={Date.UTC(2026, 9, 17, 14, 32)} now={Number.NaN} />);
    expect(container.textContent).toContain("Live by 14:32 UTC");
    expect(container.textContent).toContain("(client cache)");
  });
});

describe("a count that is not a count", () => {
  it.each<[string, unknown]>([
    ["a string", "3"],
    ["markup", "<b>3</b>"],
    ["negative", -4],
    ["fractional", 2.5],
    ["not a number", Number.NaN],
    ["infinite", Number.POSITIVE_INFINITY],
  ])("is dropped when it is %s", (_name, bad) => {
    const { container } = render(<PickupCounts counts={[["needs_notify_or_restart", bad as number]]} density="compact" />);
    expect(container.querySelector("[data-slot='pickup-chip']")).toBeNull();
    expect(container.textContent).not.toContain("<b>");
  });
});

describe("a notifications flag that is not a boolean", () => {
  it.each(MARKUP)("is 'not reported by the hub' for %j: the value is never drawn", (hostile) => {
    const { container } = render(<NotificationsNote enabled={hostile as unknown as boolean} />);
    expect(container.textContent).toBe("Notifications: not reported by the hub");
    expect(container.innerHTML).not.toContain("onerror");
    inert(container);
  });
});
