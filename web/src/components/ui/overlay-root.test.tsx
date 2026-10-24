import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Button } from "./button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "./popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";

// Found by the axe run of T4 (C1b, then C2): Radix portals an overlay to <body>, outside every landmark, and axe's `region` rule
// (all content belongs to a landmark) names the hidden `role="tooltip"` copy of ANY open tooltip, and the content of a menu or a
// popover. The fix is in the shared components: they portal into the page's one <main> landmark. `main` is a fixed-position
// overlay's containing block no more than <body> is, so nothing is placed or stacked differently.

describe("where an overlay that is not a dialog puts its content", () => {
  it("an open tooltip, with its hidden role=tooltip copy, is inside the main landmark", async () => {
    render(
      <>
        <header>
          <Button>Elsewhere</Button>
        </header>
        <main id="main">
          <Tooltip open>
            <TooltipTrigger asChild>
              <Button aria-label="Copy path" size="icon">
                c
              </Button>
            </TooltipTrigger>
            <TooltipContent>Copy path</TooltipContent>
          </Tooltip>
        </main>
      </>,
    );
    const hidden = await screen.findByRole("tooltip");
    expect(hidden.closest("main")).toBe(screen.getByRole("main"));
    for (const node of document.querySelectorAll("[data-radix-popper-content-wrapper]")) expect(node.closest("main")).not.toBeNull();
  });

  it("with no main on the page (a component test, a test page) the tooltip goes under <body> as before", async () => {
    render(
      <Tooltip open>
        <TooltipTrigger asChild>
          <Button aria-label="Copy path" size="icon">
            c
          </Button>
        </TooltipTrigger>
        <TooltipContent>Copy path</TooltipContent>
      </Tooltip>,
    );
    const hidden = await screen.findByRole("tooltip");
    expect(document.body.contains(hidden)).toBe(true);
    expect(hidden.closest("main")).toBeNull();
  });

  it("an open popover is inside the main landmark", async () => {
    const user = userEvent.setup();
    render(
      <main id="main">
        <Popover>
          <PopoverTrigger asChild>
            <Button>Columns</Button>
          </PopoverTrigger>
          <PopoverContent aria-label="Choose columns">Column chooser</PopoverContent>
        </Popover>
      </main>,
    );
    await user.click(screen.getByRole("button", { name: "Columns" }));
    expect((await screen.findByText("Column chooser")).closest("main")).toBe(screen.getByRole("main"));
  });

  it("an open menu is inside the main landmark, and the page behind it is not hidden from assistive technology (a non-modal menu)", async () => {
    const user = userEvent.setup();
    render(
      <>
        <header>
          <Button>Elsewhere</Button>
        </header>
        <main id="main">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button>Account</Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuItem>Sign out</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </main>
      </>,
    );
    await user.click(screen.getByRole("button", { name: "Account" }));
    expect((await screen.findByRole("menuitem", { name: "Sign out" })).closest("main")).toBe(screen.getByRole("main"));
    // A modal menu marks the rest of the page aria-hidden while its focusable controls stay in the tab order, which axe reports as
    // `aria-hidden-focus`. A menu button's menu is not modal in the WAI-ARIA pattern: Tab and Escape leave it.
    expect(screen.getByRole("banner")).not.toHaveAttribute("aria-hidden");
    expect(screen.getByRole("button", { name: "Elsewhere" })).toBeInTheDocument();
  });
});

describe("a popover has a name", () => {
  it("cannot be written without one (a role=dialog with no name is an axe violation)", () => {
    // @ts-expect-error a popover's content is a dialog and needs `aria-label` or `aria-labelledby`.
    const missing = <PopoverContent>no name</PopoverContent>;
    const named = <PopoverContent aria-label="Choose columns">named</PopoverContent>;
    const labelled = <PopoverContent aria-labelledby="heading-id">labelled</PopoverContent>;
    expect([missing, named, labelled]).toHaveLength(3);
  });
});
