import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Severity } from "@/api/generated/model";
import { expectNoAxeViolations } from "@/test/axe";
import { SeverityBadge } from "./SeverityBadge";
import { SeverityFilter } from "./SeverityFilter";

// The ONE severity badge of the app (the swimlane Findings tab, the Findings screen, the file history and the drift rows used
// three of their own that disagreed). Four levels. It follows the design's `.sv-*` tokens of the Findings screen: High is the
// conflict red, Medium the warning amber, Low the info blue with no fill; the contract has a fourth level, so Critical takes
// High's colour and is told from it by its shield, its word and a heavier weight. Never colour alone: an icon and a word each.

// What each level looks like, written out. A change to any of these is a change to how the whole app draws severity.
const LOOK = {
  critical: { label: "Critical", icon: "lucide-shield-alert", tone: ["border-status-conflict-border", "bg-status-conflict-bg", "text-status-conflict-fg"], weight: "font-semibold" },
  high: { label: "High", icon: "lucide-circle-x", tone: ["border-status-conflict-border", "bg-status-conflict-bg", "text-status-conflict-fg"], weight: "font-medium" },
  medium: { label: "Medium", icon: "lucide-triangle-alert", tone: ["border-warning-border", "bg-warning-bg", "text-warning-fg"], weight: "font-medium" },
  low: { label: "Low", icon: "lucide-info", tone: ["border-info-border", "bg-transparent", "text-info-fg"], weight: "font-medium" },
} as const;

describe("SeverityBadge", () => {
  it.each(Object.values(Severity))("draws %s as an icon, its word and its own colours", (severity) => {
    const { container } = render(<SeverityBadge severity={severity} />);
    const badge = container.firstElementChild as HTMLElement;
    const look = LOOK[severity];
    expect(badge).toHaveAttribute("data-slot", "severity-badge");
    expect(badge).toHaveAttribute("data-severity", severity);
    expect(badge).toHaveTextContent(look.label);
    const icon = badge.querySelector("svg");
    expect(icon).not.toBeNull();
    expect(icon).toHaveAttribute("aria-hidden", "true");
    expect(icon?.getAttribute("class")).toContain(look.icon);
    for (const token of look.tone) expect(badge.className, token).toContain(token);
    expect(badge.className).toContain(look.weight);
    // The same box as every other badge: 20 px, a 4 px radius, a 12 px icon.
    for (const part of ["h-5", "rounded-sm", "border", "text-xs", "[&_svg]:size-3"]) expect(badge.className, part).toContain(part);
  });

  it("tells every level apart without colour: by its word, and by its icon", () => {
    const words = new Set<string>();
    const icons = new Set<string>();
    for (const severity of Object.values(Severity)) {
      const { container, unmount } = render(<SeverityBadge severity={severity} />);
      words.add(container.textContent ?? "");
      icons.add(container.querySelector("svg")?.getAttribute("class") ?? "");
      unmount();
    }
    expect(words.size).toBe(4);
    expect(icons.size).toBe(4);
  });

  it("gives Critical High's colour on purpose, and tells them apart by shield, word and weight (design/DEVIATIONS.md, Findings: severity)", () => {
    const tone = (severity: Severity) => LOOK[severity].tone.join(" ");
    expect(tone("critical")).toBe(tone("high"));
    expect(LOOK.critical.icon).not.toBe(LOOK.high.icon);
    expect(LOOK.critical.weight).not.toBe(LOOK.high.weight);
  });

  it("gives Low no fill, so that it differs from Medium in greyscale as well", () => {
    const { container } = render(<SeverityBadge severity="low" />);
    expect((container.firstElementChild as HTMLElement).className).toContain("bg-transparent");
  });

  it("reads `High severity` in a header that asks for the long form, and only there", () => {
    const { container } = render(<SeverityBadge severity="high" long />);
    expect(container).toHaveTextContent("High severity");
    const short = render(<SeverityBadge severity="high" />).container;
    expect(short).not.toHaveTextContent("severity");
  });

  it("draws nothing for a row nobody rated: it never makes a severity up", () => {
    expect(render(<SeverityBadge severity={null} />).container).toBeEmptyDOMElement();
    expect(render(<SeverityBadge severity={undefined} />).container).toBeEmptyDOMElement();
  });

  it("takes a class from the caller for its place in a layout, and keeps its own colours", () => {
    const { container } = render(<SeverityBadge severity="medium" className="justify-self-start" />);
    const badge = container.firstElementChild as HTMLElement;
    expect(badge.className).toContain("justify-self-start");
    expect(badge.className).toContain("bg-warning-bg");
  });

  it("has no axe violations", async () => {
    render(
      <ul>
        {Object.values(Severity).map((s) => (
          <li key={s}>
            <SeverityBadge severity={s} />
          </li>
        ))}
      </ul>,
    );
    await expectNoAxeViolations();
  });
});

describe("SeverityFilter", () => {
  it("offers a minimum, as the Findings screen's menu does, and starts at Any", () => {
    render(<SeverityFilter value={null} onChange={() => undefined} scope="server" />);
    const select = screen.getByRole("combobox", { name: "Severity" });
    expect(select).toHaveValue("any");
    expect(Array.from((select as HTMLSelectElement).options).map((o) => o.textContent)).toEqual(["Any severity", "Medium and above", "High and above", "Critical only"]);
  });

  it("says the level that was picked, or null for Any", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeverityFilter value={null} onChange={onChange} scope="server" />);
    const select = screen.getByRole("combobox", { name: "Severity" });
    await user.selectOptions(select, "High and above");
    expect(onChange).toHaveBeenLastCalledWith("high");
    await user.selectOptions(select, "Any severity");
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it("shows the level it is given", () => {
    render(<SeverityFilter value="critical" onChange={() => undefined} scope="server" />);
    expect(screen.getByRole("combobox", { name: "Severity" })).toHaveValue("critical");
  });

  it("has no note when the server filters: the answer is the whole one, even with more pages", () => {
    render(<SeverityFilter value="high" onChange={() => undefined} scope="server" count={50} hasMore />);
    expect(screen.queryByText(/loaded so far/)).not.toBeInTheDocument();
  });

  it("says that it reached only the loaded rows when it filters them and there are more on the server", () => {
    render(<SeverityFilter value="high" onChange={() => undefined} scope="loaded" count={50} hasMore noun="file" />);
    expect(screen.getByText("Applies to the 50 files loaded so far. More are not loaded.")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Severity" })).toHaveAccessibleDescription("Applies to the 50 files loaded so far. More are not loaded.");
  });

  it("has no note when every row is loaded, or when no filter is on", () => {
    const { rerender } = render(<SeverityFilter value="high" onChange={() => undefined} scope="loaded" count={50} hasMore={false} />);
    expect(screen.queryByText(/loaded so far/)).not.toBeInTheDocument();
    rerender(<SeverityFilter value={null} onChange={() => undefined} scope="loaded" count={50} hasMore />);
    expect(screen.queryByText(/loaded so far/)).not.toBeInTheDocument();
  });

  it("sizes the whole control and not only the select, so the arrow stays on the select's right edge", () => {
    // NativeSelect's wrapper is as wide as its container, and the arrow is drawn at the wrapper's right edge: a width on the select
    // alone leaves the arrow floating to the right of it (seen in the Drift tab at 1440 px). The width belongs to a box around the wrapper.
    render(<SeverityFilter value={null} onChange={() => undefined} scope="loaded" />);
    const select = screen.getByRole("combobox", { name: "Severity" });
    const wrapper = select.closest("[data-slot='native-select-wrapper']");
    expect(wrapper?.parentElement, "a sized box holds the wrapper").toHaveClass("w-44");
    expect(select).not.toHaveClass("w-44");
  });

  it("can be disabled, and be given another name", () => {
    render(<SeverityFilter value={null} onChange={() => undefined} scope="loaded" label="Minimum severity" disabled />);
    expect(screen.getByRole("combobox", { name: "Minimum severity" })).toBeDisabled();
  });

  it("has no axe violations", async () => {
    render(<SeverityFilter value="high" onChange={() => undefined} scope="loaded" count={50} hasMore />);
    await expectNoAxeViolations();
  });
});
