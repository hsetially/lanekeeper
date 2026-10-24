import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderFile } from "../test-utils";
import { DiscardDialog } from "./DiscardDialog";

describe("DiscardDialog", () => {
  it("asks before it throws unsaved changes away, with the safe choice first", async () => {
    renderFile(<DiscardDialog open onOpenChange={() => undefined} name="a.yml" onDiscard={() => undefined} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Discard your changes?" });
    expect(dialog).toHaveTextContent("Your edits to a.yml have not been saved. Discarding them cannot be undone.");
    await waitFor(() => expect(screen.getByRole("button", { name: "Keep editing" })).toHaveFocus());
  });

  it("discards only when asked to", async () => {
    const onDiscard = vi.fn();
    const onOpenChange = vi.fn();
    renderFile(<DiscardDialog open onOpenChange={onOpenChange} name="a.yml" onDiscard={onDiscard} />);
    await userEvent.click(await screen.findByRole("button", { name: "Keep editing" }));
    expect(onDiscard).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(onDiscard).toHaveBeenCalledTimes(1);
  });
});
