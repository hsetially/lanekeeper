import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Role } from "@/api/generated/model";
import { rowById } from "@/keyboard";
import ShortcutsSheet, { SHEET } from "./ShortcutsSheet";

// The shortcuts sheet (the Shell, frames 1e and 1f): the design's four groups, drawn from the keymap rows so that a key can
// never be shown here that is not bound there, and a key the role cannot use is not listed.

describe("ShortcutsSheet", () => {
  it("is a dialog named Keyboard shortcuts, with the design's four groups and its note about sequences", () => {
    render(<ShortcutsSheet open onOpenChange={vi.fn()} role={Role.admin} />);
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    for (const title of ["Global", "Go to", "Diff navigation", "Lists and trees"]) expect(within(dialog).getByRole("heading", { name: title })).toBeInTheDocument();
    expect(within(dialog).getByText(/press the first key, release, then the second/)).toBeInTheDocument();
    expect(within(dialog).getByText(/pause while you type in a field/)).toBeInTheDocument();
  });

  it("draws the keys of each row as keycaps, and a sequence with the word then", () => {
    render(<ShortcutsSheet open onOpenChange={vi.fn()} role={Role.admin} />);
    const palette = screen.getByText("Command palette").closest("li") as HTMLElement;
    expect([...palette.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["⌘", "K"]);
    const compare = screen.getByText("Compare").closest("li") as HTMLElement;
    expect([...compare.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["g", "c"]);
    expect(compare).toHaveTextContent("then");
    const move = screen.getByText("Move selection").closest("li") as HTMLElement;
    expect([...move.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["↑", "↓"]);
  });

  it("lists the Audit shortcut for an Admin and not for a Viewer, who cannot open Audit", () => {
    const { rerender } = render(<ShortcutsSheet open onOpenChange={vi.fn()} role={Role.admin} />);
    expect(screen.getByText("Audit")).toBeInTheDocument();
    rerender(<ShortcutsSheet open onOpenChange={vi.fn()} role={Role.viewer} />);
    expect(screen.queryByText("Audit")).toBeNull();
  });

  it("names only rows that exist in the keymap, each with at least one key", () => {
    for (const group of SHEET) {
      for (const item of group.items) {
        const row = rowById(item.row);
        expect(row, item.row).toBeDefined();
        for (const index of item.bindings ?? [0]) expect(row?.bindings[index], `${item.row} binding ${index}`).toBeDefined();
      }
    }
  });

  it("closes on Escape through the dialog", () => {
    const onOpenChange = vi.fn();
    render(<ShortcutsSheet open onOpenChange={onOpenChange} role={Role.viewer} />);
    screen.getByRole("dialog").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
