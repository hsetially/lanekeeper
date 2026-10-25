import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Button } from "./button";
import { Spinner } from "./spinner";

describe("Button", () => {
  it("is a type=button button by default, so it never submits a form by accident", () => {
    render(<Button>Compare</Button>);
    const button = screen.getByRole("button", { name: "Compare" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveAttribute("data-variant", "default");
    expect(button).toHaveAttribute("data-size", "default");
  });

  it("calls onClick, and not when natively disabled", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const { rerender } = render(<Button onClick={onClick}>Save</Button>);
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(onClick).toHaveBeenCalledTimes(1);
    rerender(
      <Button onClick={onClick} disabled>
        Save
      </Button>,
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("loading shows a spinner, sets aria-busy and ignores clicks but keeps keyboard focus", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(
      <Button loading onClick={onClick}>
        Working…
      </Button>,
    );
    const button = screen.getByRole("button", { name: "Working…" });
    expect(button).toHaveAttribute("aria-busy", "true");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).not.toBeDisabled();
    expect(button.querySelector("svg.animate-lk-spin")).not.toBeNull();
    await user.click(button);
    await user.keyboard("{Enter}");
    expect(onClick).not.toHaveBeenCalled();
    button.focus();
    expect(button).toHaveFocus();
  });

  it("can render as the child element, for links styled as buttons", () => {
    render(
      <Button asChild variant="secondary">
        <a href="/compare">Compare</a>
      </Button>,
    );
    const link = screen.getByRole("link", { name: "Compare" });
    expect(link).toHaveAttribute("href", "/compare");
    expect(link).toHaveAttribute("data-variant", "secondary");
  });

  it("has the four handoff variants and four sizes, and no others", () => {
    for (const variant of ["default", "secondary", "ghost", "destructive"] as const) {
      const { unmount } = render(<Button variant={variant}>x</Button>);
      expect(screen.getByRole("button")).toHaveAttribute("data-variant", variant);
      unmount();
    }
    for (const size of ["default", "sm", "icon", "icon-sm"] as const) {
      const { unmount } = render(
        <Button size={size} aria-label="x">
          <Spinner />
        </Button>,
      );
      expect(screen.getByRole("button")).toHaveAttribute("data-size", size);
      unmount();
    }
  });
});

describe("Spinner", () => {
  it("is decorative and animates with the reduced-motion aware class", () => {
    const { container } = render(<Spinner />);
    const svg = container.querySelector("svg");
    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(svg).toHaveClass("animate-lk-spin");
  });
});
