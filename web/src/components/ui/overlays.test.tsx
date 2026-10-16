import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogTitle, AlertDialogTrigger } from "./alert-dialog";
import { Button } from "./button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle, DialogTrigger } from "./dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "./dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "./popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";

describe("Dialog", () => {
  function Example() {
    return (
      <Dialog>
        <DialogTrigger asChild>
          <Button>Shortcuts</Button>
        </DialogTrigger>
        <DialogContent>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>Press ? anywhere to open this sheet.</DialogDescription>
          <DialogFooter>
            <Button variant="secondary">Done</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }

  it("is named by its title, described by its description, and has no axe violations", async () => {
    const user = userEvent.setup();
    render(<Example />);
    await user.click(screen.getByRole("button", { name: "Shortcuts" }));
    const dialog = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog).toHaveAccessibleDescription("Press ? anywhere to open this sheet.");
    await expectNoAxeViolations();
  });

  it("closes with Escape and returns focus to the trigger", async () => {
    const user = userEvent.setup();
    render(<Example />);
    const trigger = screen.getByRole("button", { name: "Shortcuts" });
    await user.click(trigger);
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(trigger).toHaveFocus();
  });

  it("has a close button with a name", async () => {
    const user = userEvent.setup();
    render(<Example />);
    await user.click(screen.getByRole("button", { name: "Shortcuts" }));
    await user.click(await screen.findByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });
});

describe("AlertDialog", () => {
  it("puts initial focus on Cancel, the least destructive control", async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    render(
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="destructive">Delete file</Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogTitle>Delete this file?</AlertDialogTitle>
          <AlertDialogDescription>The file becomes NFS ahead until a PR merges.</AlertDialogDescription>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onAction}>Delete file</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>,
    );
    await user.click(screen.getByRole("button", { name: "Delete file" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete this file?" });
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    await expectNoAxeViolations();
    await user.click(within(dialog).getByRole("button", { name: "Delete file" }));
    expect(onAction).toHaveBeenCalledTimes(1);
  });
});

describe("Tooltip", () => {
  it("opens on keyboard focus and closes with Escape", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip>
        <TooltipTrigger asChild>
          <Button aria-label="Copy path" size="icon">
            c
          </Button>
        </TooltipTrigger>
        <TooltipContent>Copy path</TooltipContent>
      </Tooltip>,
    );
    await user.tab();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Copy path");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument());
  });
});

describe("Popover", () => {
  it("opens, closes with Escape and returns focus", async () => {
    const user = userEvent.setup();
    render(
      <Popover>
        <PopoverTrigger asChild>
          <Button>Columns</Button>
        </PopoverTrigger>
        <PopoverContent aria-label="Choose columns">Column chooser</PopoverContent>
      </Popover>,
    );
    const trigger = screen.getByRole("button", { name: "Columns" });
    await user.click(trigger);
    expect(await screen.findByText("Column chooser")).toBeVisible();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByText("Column chooser")).not.toBeInTheDocument());
    expect(trigger).toHaveFocus();
  });
});

describe("DropdownMenu", () => {
  it("opens from the keyboard, moves with the arrows and runs the chosen item", async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    render(
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button>Marco Diaz</Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuLabel>Signed in as marco</DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem>My GitHub token</DropdownMenuItem>
          <DropdownMenuItem onSelect={onSignOut}>Sign out</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>,
    );
    await user.tab();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("menu")).toBeVisible();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Sign out" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onSignOut).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
  });
});
