import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandShortcut } from "./command";

function Palette({ onSelect }: { onSelect: (v: string) => void }) {
  return (
    <Command label="Command palette">
      <CommandInput placeholder="Search swimlanes, files, settings…" />
      <CommandList>
        <CommandEmpty>No results</CommandEmpty>
        <CommandGroup heading="Swimlanes">
          <CommandItem value="sitb" onSelect={onSelect}>
            sitb
            <CommandShortcut>⌘1</CommandShortcut>
          </CommandItem>
          <CommandItem value="presita" onSelect={onSelect}>
            presita
          </CommandItem>
        </CommandGroup>
      </CommandList>
    </Command>
  );
}

describe("Command", () => {
  it("filters as you type and runs the active item on Enter", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Palette onSelect={onSelect} />);
    expect(screen.getAllByRole("option")).toHaveLength(2);
    await user.type(screen.getByRole("combobox"), "pre");
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith("presita");
  });

  it("says so when nothing matches", async () => {
    const user = userEvent.setup();
    render(<Palette onSelect={() => {}} />);
    await user.type(screen.getByRole("combobox"), "zzz");
    expect(await screen.findByText("No results")).toBeVisible();
  });

  it("when nothing matches, the list is out of the accessibility tree and the message is a status beside it, not a child of it (axe `aria-required-children`)", async () => {
    const user = userEvent.setup();
    render(<Palette onSelect={() => {}} />);
    await user.type(screen.getByRole("combobox"), "zzz");
    const message = await screen.findByText("No results");
    // An empty listbox, or a listbox with a message in it, is an axe violation; so the list is hidden and the message is beside it.
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(message.closest("[cmdk-list]")).toBeNull();
    expect(message.closest("[role='status']")).not.toBeNull();
    await expectNoAxeViolations();
    // And it comes back when the filter does.
    await user.clear(screen.getByRole("combobox"));
    await waitFor(() => expect(screen.getByRole("listbox")).toBeVisible());
    expect(screen.queryByText("No results")).toBeNull();
  });

  it("CommandDialog is a named dialog with a hidden title", async () => {
    render(
      <CommandDialog open onOpenChange={() => {}} title="Command palette" description="Search and run anything">
        <CommandInput placeholder="Search" />
        <CommandList>
          <CommandGroup heading="Actions">
            <CommandItem value="a">Open</CommandItem>
          </CommandGroup>
        </CommandList>
      </CommandDialog>,
    );
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
    await expectNoAxeViolations();
  });
});
