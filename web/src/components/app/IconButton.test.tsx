import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Search } from "@/icons";
import { IconButton } from "./IconButton";

describe("IconButton", () => {
  it("is named by its label, and shows the label with the shortcut hint in a tooltip on focus", async () => {
    const user = userEvent.setup();
    render(
      <IconButton label="Search text" shortcut="/">
        <Search />
      </IconButton>,
    );
    const button = screen.getByRole("button", { name: "Search text" });
    await user.tab();
    expect(button).toHaveFocus();
    const tip = await screen.findByRole("tooltip");
    expect(tip).toHaveTextContent("Search text");
    expect(tip).toHaveTextContent("/");
  });

  it("is 28px by default and 24px when small, and runs onClick", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const { rerender } = render(
      <IconButton label="Copy" onClick={onClick}>
        <Search />
      </IconButton>,
    );
    expect(screen.getByRole("button", { name: "Copy" })).toHaveAttribute("data-size", "icon");
    await user.click(screen.getByRole("button", { name: "Copy" }));
    expect(onClick).toHaveBeenCalledTimes(1);
    rerender(
      <IconButton label="Copy" size="icon-sm">
        <Search />
      </IconButton>,
    );
    expect(screen.getByRole("button", { name: "Copy" })).toHaveAttribute("data-size", "icon-sm");
  });

  it("supports the pressed state of a toggle", () => {
    render(
      <IconButton label="Match case" pressed>
        <Search />
      </IconButton>,
    );
    expect(screen.getByRole("button", { name: "Match case" })).toHaveAttribute("aria-pressed", "true");
  });
});
