import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { Banner } from "./Banner";

// A banner is a status, not a toast: it says what is the matter, what it turns off and what to do, with a colour, an icon and words.

describe("Banner", () => {
  it.each(["info", "success", "warning"] as const)("%s is a polite status with an icon", (variant) => {
    render(
      <Banner variant={variant} title="Something">
        What it means.
      </Banner>,
    );
    const banner = screen.getByRole("status");
    expect(banner).toHaveAttribute("data-variant", variant);
    expect(banner.querySelector("svg")).not.toBeNull();
  });

  it("danger is an alert", () => {
    render(<Banner variant="danger" title="Broken" />);
    expect(screen.getByRole("alert")).toHaveAttribute("data-variant", "danger");
  });

  it("locked is the design's own family (the lock colours), a polite status with a lock, the title, the words and the action", async () => {
    render(
      <Banner variant="locked" title="Editing locked" action={<a href="/adopt">Start adoption</a>}>
        sitb has no confirmed baseline.
      </Banner>,
    );
    const banner = screen.getByRole("status");
    expect(banner).toHaveAttribute("data-variant", "locked");
    expect(banner).toHaveClass("bg-status-locked-bg", "text-status-locked-fg", "border-status-locked-border");
    expect(banner.querySelector("svg.lucide-lock")).not.toBeNull();
    expect(banner).toHaveTextContent("Editing locked");
    expect(banner).toHaveTextContent("sitb has no confirmed baseline.");
    expect(screen.getByRole("link", { name: "Start adoption" })).toBeInTheDocument();
    await expectNoAxeViolations();
  });

  it("puts the action beside the words, in one row: the design's banner is one line with its button at the right", () => {
    render(
      <Banner variant="warning" title="Agent offline since 14:05" action={<button type="button">Check agent</button>}>
        Restarts are off until the agent reconnects.
      </Banner>,
    );
    // The Alert it is made of lays its children out in a column, so the row has to be one child of it: a text block and the action in it.
    const row = screen.getByRole("button", { name: "Check agent" }).closest("[data-slot='banner-row']");
    expect(row).not.toBeNull();
    expect(row).toContainElement(screen.getByText("Agent offline since 14:05"));
    expect(row).not.toHaveClass("flex-col");
    expect(screen.getByRole("status").querySelector(":scope > div")?.contains(row)).toBe(true);
  });

  it("lets the action drop to a line of its own, to the right, when the words would be squeezed to nothing beside it (a pane of 360 px with a long button, labels 40% longer: found by the layout run)", () => {
    render(
      <Banner variant="warning" title="Config-server restart needed" action={<button type="button">Restart config-server</button>}>
        2 services report needing a config-server restart.
      </Banner>,
    );
    const row = screen.getByRole("button", { name: "Restart config-server" }).closest<HTMLElement>("[data-slot='banner-row']");
    if (row === null) throw new Error("no banner row");
    // The row wraps, and the words have a width they keep (the smaller of a line's worth and the whole row), so the button is what moves.
    expect(row).toHaveClass("flex-wrap");
    const words = row.firstElementChild as HTMLElement;
    expect(words.className).toMatch(/min-w-\[min\(100%,14rem\)\]/);
    const action = screen.getByRole("button", { name: "Restart config-server" }).parentElement as HTMLElement;
    expect(action).toHaveClass("ml-auto");
  });

  it("can be dismissed only when it is a success", () => {
    const { rerender } = render(<Banner variant="success" title="Saved" onDismiss={() => undefined} />);
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeInTheDocument();
    rerender(<Banner variant="locked" title="Editing locked" onDismiss={() => undefined} />);
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();
  });
});
