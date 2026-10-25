import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { Input } from "@/components/ui/input";
import { ConsequenceDialog } from "./ConsequenceDialog";
import { DestructiveConfirm } from "./DestructiveConfirm";
import { ProblemDialog } from "./ProblemDialog";

function Consequence({ children, ...props }: Partial<React.ComponentProps<typeof ConsequenceDialog>> = {}) {
  return (
    <ConsequenceDialog
      open
      onOpenChange={() => {}}
      title="Raise PR"
      consequence="The file becomes NFS ahead until a PR merges."
      actionLabel="Raise PR"
      onConfirm={() => {}}
      {...props}
    >
      {children === undefined ? (
        <label>
          PR title
          <Input defaultValue="Update core" />
        </label>
      ) : (
        children
      )}
    </ConsequenceDialog>
  );
}

describe("ConsequenceDialog", () => {
  it("states the consequence first and is named by its title, with no axe violations", async () => {
    render(<Consequence />);
    const dialog = await screen.findByRole("alertdialog", { name: "Raise PR" });
    expect(dialog).toHaveAccessibleDescription("The file becomes NFS ahead until a PR merges.");
    const order = Array.from(dialog.querySelectorAll("[data-slot=alert-dialog-description], input")).map((n) => n.tagName);
    expect(order[0]).toBe("P");
    await expectNoAxeViolations();
  });

  it("confirms with the action verb, and puts initial focus on Cancel", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Consequence onConfirm={onConfirm} />);
    expect(await screen.findByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Raise PR" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("puts initial focus on the first field when it is a form to fill in (the design's safe dialogs), and on Cancel otherwise", async () => {
    render(<Consequence initialFocus="first-field" />);
    expect(await screen.findByLabelText("PR title")).toHaveFocus();
  });

  it("falls back to Cancel when a dialog asked for the first field has none", async () => {
    render(<Consequence initialFocus="first-field">{null}</Consequence>);
    expect(await screen.findByRole("button", { name: "Cancel" })).toHaveFocus();
  });

  it("skips a disabled field, and finds a radio in a group", async () => {
    render(
      <Consequence initialFocus="first-field">
        <input aria-label="Locked" disabled />
        <div role="radiogroup" aria-label="Role">
          <button type="button" role="radio" aria-checked="false" tabIndex={-1}>
            Viewer
          </button>
          <button type="button" role="radio" aria-checked="true" tabIndex={0}>
            Editor
          </button>
        </div>
      </Consequence>,
    );
    expect(await screen.findByRole("radio", { name: "Editor" })).toHaveFocus();
  });

  it("refuses 'OK' as the action label: the button names the action", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<Consequence actionLabel="OK" />)).toThrow(/verb/i);
    quiet.mockRestore();
  });

  it("Enter in a field confirms only when the primary action is enabled", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const { rerender } = render(<Consequence onConfirm={onConfirm} />);
    await user.type(screen.getByLabelText("PR title"), "{Enter}");
    expect(onConfirm).toHaveBeenCalledTimes(1);
    rerender(<Consequence onConfirm={onConfirm} canConfirm={false} />);
    await user.type(screen.getByLabelText("PR title"), "{Enter}");
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Raise PR" })).toHaveAttribute("aria-disabled", "true");
  });

  it("while working: spinner label, no second submit, Cancel disabled, Escape does not close", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onOpenChange = vi.fn();
    render(<Consequence working workingLabel="Raising…" onConfirm={onConfirm} onOpenChange={onOpenChange} />);
    const primary = await screen.findByRole("button", { name: "Raising…" });
    expect(primary).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await user.click(primary);
    await user.type(screen.getByLabelText("PR title"), "{Enter}");
    await user.keyboard("{Escape}");
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("on error keeps the dialog open with the user's input and shows what happened with a request ID", async () => {
    const user = userEvent.setup();
    render(<Consequence error={{ title: "The PR was not created", detail: "GitHub refused the token.", requestId: "req_42" }} />);
    await user.type(screen.getByLabelText("PR title"), " v2");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The PR was not created");
    expect(alert).toHaveTextContent("req_42");
    expect(screen.getByLabelText("PR title")).toHaveValue("Update core v2");
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });

  it("closes through onOpenChange when Cancel is pressed", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    render(<Consequence onOpenChange={onOpenChange} />);
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe("DestructiveConfirm", () => {
  const NAME = "tx-infinity-core-sit1.yml";
  function Destructive(props: Partial<React.ComponentProps<typeof DestructiveConfirm>> = {}) {
    return (
      <DestructiveConfirm
        open
        onOpenChange={() => {}}
        title="Delete file"
        consequence="The file becomes NFS ahead until a PR merges."
        confirmValue={NAME}
        actionLabel="Delete file"
        onConfirm={() => {}}
        {...props}
      />
    );
  }

  it("puts initial focus on Cancel and keeps Delete disabled until the exact name is typed", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Destructive onConfirm={onConfirm} />);
    expect(await screen.findByRole("button", { name: "Cancel" })).toHaveFocus();
    const confirm = screen.getByRole("button", { name: "Delete file" });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    const field = screen.getByLabelText(/Type .* to confirm/);
    await user.type(field, NAME.toUpperCase());
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await user.clear(field);
    await user.type(field, `${NAME} `);
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await user.clear(field);
    await user.type(field, NAME);
    expect(confirm).not.toHaveAttribute("aria-disabled");
    await user.click(confirm);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("holds what the caller puts between the consequence and the name field (an effect table, a note)", async () => {
    render(
      <Destructive>
        <p>Who reads this file</p>
      </Destructive>,
    );
    const dialog = await screen.findByRole("alertdialog", { name: "Delete file" });
    const note = within(dialog).getByText("Who reads this file");
    const description = dialog.querySelector("[data-slot=alert-dialog-description]");
    const field = within(dialog).getByLabelText(/Type .* to confirm/);
    expect(description?.compareDocumentPosition(note)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(note.compareDocumentPosition(field)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("stays off while the caller says the write is not ready, even with the name typed in full", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const { rerender } = render(<Destructive canConfirm={false} onConfirm={onConfirm} />);
    await user.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    const confirm = screen.getByRole("button", { name: "Delete file" });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await user.click(confirm);
    expect(onConfirm).not.toHaveBeenCalled();
    rerender(<Destructive canConfirm onConfirm={onConfirm} />);
    expect(screen.getByRole("button", { name: "Delete file" })).not.toHaveAttribute("aria-disabled");
  });

  it("says what Enter does and when the name matches, when asked to (the design's delete frames 2a and 2b)", async () => {
    const user = userEvent.setup();
    render(<Destructive showMatchHint />);
    expect(await screen.findByText("Enter does nothing until the name matches")).toBeInTheDocument();
    await user.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    expect(screen.getByText("Name matches")).toBeInTheDocument();
  });

  it("draws the title and the name as data, and its icons are decoration", async () => {
    render(<Destructive title={"Delete a‮b.yml"} titleIcon={<svg data-testid="t-icon" />} actionIcon={<svg data-testid="a-icon" />} />);
    expect((await screen.findAllByText("<U+202E>")).length).toBeGreaterThan(0);
    expect(screen.getByTestId("t-icon").parentElement).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByTestId("a-icon").parentElement).toHaveAttribute("aria-hidden", "true");
  });

  it("never confirms on Enter, even with the right name typed", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Destructive onConfirm={onConfirm} />);
    await user.type(screen.getByLabelText(/Type .* to confirm/), `${NAME}{Enter}`);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("never confirms on Enter pressed on the Delete button itself (a button's own Enter is a click); Space and a click do", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Destructive onConfirm={onConfirm} />);
    await user.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    const confirm = screen.getByRole("button", { name: "Delete file" });
    confirm.focus();
    await user.keyboard("{Enter}");
    expect(onConfirm).not.toHaveBeenCalled();
    await user.keyboard(" ");
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("does nothing when a disabled Delete is clicked", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<Destructive onConfirm={onConfirm} />);
    await user.click(screen.getByRole("button", { name: "Delete file" }));
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("shows the name with hidden characters made visible, and a copy button for a name that cannot be typed", async () => {
    render(<Destructive confirmValue={"a\u200Bb.yml"} />);
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("<U+200B>")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Copy name" })).toBeInTheDocument();
  });

  it("while working: Cancel is disabled, Escape does not close, and a second click does not send again", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onOpenChange = vi.fn();
    render(<Destructive working workingLabel="Deleting…" onConfirm={onConfirm} onOpenChange={onOpenChange} />);
    const primary = await screen.findByRole("button", { name: "Deleting…" });
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await user.click(primary);
    await user.keyboard("{Escape}");
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("shows work in progress and errors like the consequence dialog", async () => {
    const { rerender } = render(<Destructive working workingLabel="Deleting…" />);
    expect(await screen.findByRole("button", { name: "Deleting…" })).toHaveAttribute("aria-busy", "true");
    rerender(<Destructive error={{ title: "Delete failed", requestId: "req_9" }} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("req_9");
    await expectNoAxeViolations();
  });
});

describe("ProblemDialog", () => {
  it.each([
    ["conflict", /Someone else changed it first/],
    ["locked", /Editing is locked/],
    ["approval", /Sent for approval/],
  ] as const)("%s has its own default title", async (variant, title) => {
    render(<ProblemDialog open onOpenChange={() => {}} variant={variant} />);
    expect(await screen.findByRole("dialog", { name: title })).toBeInTheDocument();
  });

  it("explains what happened, shows the request ID, the extra detail and the actions given", async () => {
    render(
      <ProblemDialog
        open
        onOpenChange={() => {}}
        variant="locked"
        description="The baseline is not confirmed yet."
        requestId="req_77"
        actions={<button type="button">Open adoption</button>}
      >
        <p>Held by Marco Diaz since 10:42.</p>
      </ProblemDialog>,
    );
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveAccessibleDescription("The baseline is not confirmed yet.");
    expect(within(dialog).getByText("Held by Marco Diaz since 10:42.")).toBeInTheDocument();
    expect(within(dialog).getByText("req_77")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Open adoption" })).toBeInTheDocument();
  });

  it("opens with focus on the dialog itself, not on the request ID's copy button or on an action, so Escape closes it at once", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    render(
      <ProblemDialog open onOpenChange={onOpenChange} variant="conflict" requestId="req_77" actions={<button type="button">Discard my changes</button>}>
        <p>Held by Marco Diaz since 10:42.</p>
      </ProblemDialog>,
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog).toHaveFocus());
    // No hint tooltip opened on a button nobody pointed at or tabbed to: it would take the first Escape.
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.keyboard("{Escape}");
    expect(onOpenChange).toHaveBeenCalledWith(false);
    // The request ID is the first stop of the keyboard, and copies from there.
    await user.tab();
    expect(within(dialog).getByRole("button", { name: "Copy request ID" })).toHaveFocus();
  });

  it("has one Close button, not two: the footer's, with no corner cross beside it", async () => {
    const onOpenChange = vi.fn();
    render(<ProblemDialog open onOpenChange={onOpenChange} variant="conflict" />);
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("has no corner cross when the caller's actions carry their own Close, and has one when the caller asks for it", async () => {
    const own = render(
      <ProblemDialog
        open
        onOpenChange={() => {}}
        variant="locked"
        actions={
          <>
            <button type="button">Open adoption</button>
            <button type="button">Close</button>
          </>
        }
      />,
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getAllByRole("button", { name: "Close" })).toHaveLength(1);
    own.unmount();
    render(<ProblemDialog open onOpenChange={() => {}} variant="locked" actions={<button type="button">Open adoption</button>} showCloseButton />);
    expect(within(await screen.findByRole("dialog")).getAllByRole("button", { name: "Close" })).toHaveLength(1);
  });

  it("links to the proposal for 'sent for approval'", async () => {
    render(
      <ProblemDialog open onOpenChange={() => {}} variant="approval">
        <a href="/approvals/12">View proposal</a>
      </ProblemDialog>,
    );
    expect(await screen.findByRole("link", { name: "View proposal" })).toHaveAttribute("href", "/approvals/12");
  });

  it("closes with Escape and has no axe violations", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    render(<ProblemDialog open onOpenChange={onOpenChange} variant="conflict" />);
    await screen.findByRole("dialog");
    await expectNoAxeViolations();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
});
