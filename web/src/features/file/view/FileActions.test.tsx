import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderFile } from "../test-utils";
import { FileActions } from "./FileActions";
import type { FileGates } from "./gates";

const allowed = { allowed: true, reason: null };
const open: FileGates = { edit: allowed, upload: allowed, delete: allowed, revert: allowed, pr: allowed };
const handlers = { onEdit: vi.fn(), onPr: vi.fn(), onUpload: vi.fn(), onDelete: vi.fn(), onHistory: vi.fn(), onDocs: vi.fn() };
const base = { gates: open, editing: false, withheld: false, historyOpen: false, docs: "available" as const, ...handlers };

describe("FileActions", () => {
  it("offers Edit, Raise PR, Upload, Delete, History and Docs for this file", () => {
    renderFile(<FileActions {...base} />);
    return Promise.all(["Edit", "Raise PR", "Upload", "Delete", "History", "Docs for this file"].map((name) => screen.findByRole("button", { name }))).then((all) => expect(all).toHaveLength(6));
  });

  it("calls the handler of the button pressed", async () => {
    renderFile(<FileActions {...base} />);
    await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await userEvent.click(screen.getByRole("button", { name: "Raise PR" }));
    await userEvent.click(screen.getByRole("button", { name: "Upload" }));
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect([handlers.onEdit, handlers.onPr, handlers.onUpload, handlers.onDelete].map((f) => f.mock.calls.length)).toEqual([1, 1, 1, 1]);
  });

  it("disables a button it may not press, keeps it focusable and gives the reason", async () => {
    const gates: FileGates = { ...open, edit: { allowed: false, reason: "Needs the Editor role. Ask an Admin to grant it." } };
    handlers.onEdit.mockClear();
    renderFile(<FileActions {...base} gates={gates} />);
    const edit = await screen.findByRole("button", { name: "Edit" });
    expect(edit).toHaveAttribute("aria-disabled", "true");
    expect(edit).toHaveAccessibleDescription("Needs the Editor role. Ask an Admin to grant it.");
    await userEvent.click(edit);
    expect(handlers.onEdit).not.toHaveBeenCalled();
  });

  it("disables Edit while the file is being edited", async () => {
    renderFile(<FileActions {...base} editing />);
    expect(await screen.findByRole("button", { name: "Edit" })).toHaveAttribute("aria-disabled", "true");
  });

  it("shows only History for a file whose content is withheld", async () => {
    renderFile(<FileActions {...base} withheld docs="none" />);
    await screen.findByRole("button", { name: "History" });
    for (const name of ["Edit", "Raise PR", "Upload", "Delete", "Docs for this file"]) expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
  });

  it("shows Docs for this file only when the file has docs", async () => {
    renderFile(<FileActions {...base} docs="none" />);
    await screen.findByRole("button", { name: "History" });
    expect(screen.queryByRole("button", { name: "Docs for this file" })).not.toBeInTheDocument();
  });

  it("says which panel is open", async () => {
    renderFile(<FileActions {...base} historyOpen docs="open" />);
    expect(await screen.findByRole("button", { name: "History" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Docs for this file" })).toHaveAttribute("aria-pressed", "true");
  });
});
