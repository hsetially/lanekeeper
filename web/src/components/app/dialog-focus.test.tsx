import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef, useState } from "react";
import { describe, expect, it } from "vitest";
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ConsequenceDialog } from "./ConsequenceDialog";
import { DestructiveConfirm } from "./DestructiveConfirm";

// Where focus goes when a dialog closes (WCAG 2.4.3, the keymap's "focus returns to the control that opened it"). Radix gives it
// back to the dialog's own Trigger and to nothing else, but nearly every dialog of the app is opened by a plain button, a row or a
// menu choice with `open` held in state, so there is no Trigger and focus would fall to the page. The shared dialogs remember the
// control that had focus when they opened and give it back, or give it to a ref the caller names.

function Opener({ kind, returnFocus = false, firstField = false }: { kind: "consequence" | "destructive" | "plain"; returnFocus?: boolean; firstField?: boolean }) {
  const [open, setOpen] = useState(false);
  const elsewhere = useRef<HTMLButtonElement>(null);
  const ref = returnFocus ? { returnFocusRef: elsewhere } : {};
  return (
    <div>
      <button type="button" onClick={() => setOpen(true)}>
        Open it
      </button>
      <button type="button" ref={elsewhere}>
        Somewhere else
      </button>
      {kind === "consequence" ? (
        <ConsequenceDialog open={open} onOpenChange={setOpen} title="Raise PR" consequence="Makes a PR." actionLabel="Raise PR" onConfirm={() => setOpen(false)} {...ref} {...(firstField ? { initialFocus: "first-field" as const } : {})}>
          <label>
            Title
            <Input defaultValue="x" />
          </label>
        </ConsequenceDialog>
      ) : kind === "destructive" ? (
        <DestructiveConfirm open={open} onOpenChange={setOpen} title="Delete file" consequence="Gone." confirmValue="a.yml" actionLabel="Delete file" onConfirm={() => setOpen(false)} {...ref} />
      ) : (
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogContent>
            <DialogTitle>Plain</DialogTitle>
            <DialogDescription>A dialog.</DialogDescription>
          </DialogContent>
        </Dialog>
      )}
    </div>
  );
}

const opener = () => screen.getByRole("button", { name: "Open it" });

describe.each([
  ["ConsequenceDialog", "consequence", "Cancel"],
  ["DestructiveConfirm", "destructive", "Cancel"],
] as const)("%s", (_name, kind, close) => {
  it("gives focus back to the plain button that opened it, when Cancel closes it", async () => {
    const user = userEvent.setup();
    render(<Opener kind={kind} />);
    await user.click(opener());
    await user.click(await screen.findByRole("button", { name: close }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(opener()).toHaveFocus();
  });

  it("gives focus back to it when Escape closes it", async () => {
    const user = userEvent.setup();
    render(<Opener kind={kind} />);
    await user.click(opener());
    await screen.findByRole("alertdialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(opener()).toHaveFocus();
  });

  it("gives focus to the element the caller names instead, when it names one", async () => {
    const user = userEvent.setup();
    render(<Opener kind={kind} returnFocus />);
    await user.click(opener());
    await user.click(await screen.findByRole("button", { name: close }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Somewhere else" })).toHaveFocus();
  });
});

describe("ConsequenceDialog that starts in its first field", () => {
  it("still remembers the control that opened it, not the field focus moved to", async () => {
    const user = userEvent.setup();
    render(<Opener kind="consequence" firstField />);
    await user.click(opener());
    expect(await screen.findByLabelText("Title")).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(opener()).toHaveFocus();
  });
});

describe("a dialog on the plain Dialog", () => {
  it("gives focus back to the plain button that opened it", async () => {
    const user = userEvent.setup();
    render(<Opener kind="plain" />);
    await user.click(opener());
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(opener()).toHaveFocus();
  });

  it("still gives it back to its own Trigger", async () => {
    const user = userEvent.setup();
    render(
      <Dialog>
        <DialogTrigger asChild>
          <button type="button">Trigger</button>
        </DialogTrigger>
        <DialogContent>
          <DialogTitle>With a trigger</DialogTitle>
          <DialogDescription>A dialog.</DialogDescription>
        </DialogContent>
      </Dialog>,
    );
    await user.click(screen.getByRole("button", { name: "Trigger" }));
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Trigger" })).toHaveFocus();
  });

  it("lets a caller's own onCloseAutoFocus win, and does not move focus after it", async () => {
    const user = userEvent.setup();
    let calls = 0;
    function Own() {
      const [open, setOpen] = useState(false);
      return (
        <div>
          <button type="button" onClick={() => setOpen(true)}>
            Open it
          </button>
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogContent
              onCloseAutoFocus={(event) => {
                calls += 1;
                event.preventDefault();
              }}
            >
              <DialogTitle>Own</DialogTitle>
              <DialogDescription>A dialog.</DialogDescription>
            </DialogContent>
          </Dialog>
        </div>
      );
    }
    render(<Own />);
    await user.click(opener());
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls).toBe(1);
    expect(opener()).not.toHaveFocus();
  });
});

describe("when the control that opened the dialog is gone by the time it closes", () => {
  it("does not throw and does not leave focus on a node that is no longer in the page", async () => {
    const user = userEvent.setup();
    function Row() {
      const [open, setOpen] = useState(false);
      const [rowShown, setRowShown] = useState(true);
      return (
        <div>
          {rowShown ? (
            <button type="button" onClick={() => setOpen(true)}>
              Open it
            </button>
          ) : null}
          <ConsequenceDialog
            open={open}
            onOpenChange={setOpen}
            title="Remove"
            consequence="Removes the row."
            actionLabel="Remove row"
            onConfirm={() => {
              setRowShown(false);
              setOpen(false);
            }}
          />
        </div>
      );
    }
    render(<Row />);
    await user.click(opener());
    await user.click(await screen.findByRole("button", { name: "Remove row" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(document.body.contains(document.activeElement)).toBe(true);
  });
});
