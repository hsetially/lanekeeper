import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ChangePreview } from "@/api/generated/model";
import { HASH_A, HASH_B, json, problem, renderFile } from "../test-utils";
import { DeleteDialog } from "./DeleteDialog";

// Delete is the one destructive file action (2a, 2b): the effect first, from the engine, then the file's name typed in full.
// Enter never confirms it, the button stays off until the name matches, and the delete carries the preview's token and the
// hash it expects. (This follows DestructiveConfirm's rules but holds the effect table in the dialog, as the design draws it.)

const NAME = "tx-infinity-core-sit7.yml";
const PREVIEW: ChangePreview = {
  preview_id: "prev-del",
  expires_at: Date.UTC(2026, 9, 17, 10, 52),
  scope: { kind: "application", app: "tx-infinity-core" },
  warnings: [{ kind: "falls_back_to_base" }],
  changes: [{ change: { path: "tx.core.pool.maxSize", kind: "changed", left: { text: "40", ty: "int", flag: null }, right: { text: "25", ty: "int", flag: null }, left_location: null, right_location: null, severity: null }, flagged: false }],
  changes_total: 1,
  effective_changed: true,
};
const props = { open: true, onOpenChange: vi.fn(), lane: "sitb", path: `tx-infinity-api/${NAME}`, name: NAME, baseHash: HASH_A };

const server = (over: { preview?: () => Response; remove?: () => Response } = {}) => (call: { method: string; path: string }) => {
  if (call.path.endsWith("/files/preview")) return over.preview?.() ?? json(PREVIEW);
  if (call.method === "DELETE") return over.remove?.() ?? json({ outcome: "applied", hash: null, audit_id: 8 });
  return json({ items: [], next: null });
};

async function ready() {
  await screen.findByRole("table", { name: "What changes in the effective config" });
}

describe("DeleteDialog", () => {
  it("asks the engine what deleting would do, as soon as it opens, for this hash", async () => {
    const { calls } = renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await ready();
    const sent = JSON.parse(calls.find((c) => c.path.endsWith("/files/preview"))?.body ?? "{}") as Record<string, string>;
    expect(sent).toEqual({ action: "delete", path: props.path, expected_state: "hash", expected_hash: HASH_A });
  });

  it("puts the effect first: the consequence, the warning, who reads it and what changes (2a)", async () => {
    renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    const dialog = await screen.findByRole("alertdialog", { name: `Delete ${NAME} from sitb?` });
    await ready();
    expect(within(dialog).getByText(/leaves the sitb NFS share now/)).toBeInTheDocument();
    expect(within(dialog).getByText("This tenant file is removed, so the services fall back to the base file.")).toBeInTheDocument();
    expect(within(dialog).getByText("The application tx-infinity-core reads this file.")).toBeInTheDocument();
    expect(within(dialog).getByText(/The file stays in Git/)).toBeInTheDocument();
  });

  it("starts with focus on Cancel", async () => {
    renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await screen.findByRole("alertdialog");
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());
  });

  it("keeps Delete file off until the name is typed in full, and says what Enter does (2a)", async () => {
    renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await ready();
    const button = screen.getByRole("button", { name: "Delete file" });
    const field = screen.getByLabelText(/Type .* to confirm/);
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Enter does nothing until the name matches")).toBeInTheDocument();
    await userEvent.type(field, "tx-infinity-core-s");
    expect(button).toHaveAttribute("aria-disabled", "true");
  });

  it("enables Delete file when the name matches exactly (2b)", async () => {
    renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await ready();
    await userEvent.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    expect(screen.getByRole("button", { name: "Delete file" })).not.toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Name matches")).toBeInTheDocument();
  });

  it("does not take a name that only looks the same", async () => {
    renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await ready();
    await userEvent.type(screen.getByLabelText(/Type .* to confirm/), `${NAME} `);
    expect(screen.getByRole("button", { name: "Delete file" })).toHaveAttribute("aria-disabled", "true");
  });

  it("never confirms on Enter: not in the field, not on the button", async () => {
    const { calls } = renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await ready();
    const field = screen.getByLabelText(/Type .* to confirm/);
    await userEvent.type(field, `${NAME}{Enter}`);
    const button = screen.getByRole("button", { name: "Delete file" });
    button.focus();
    await userEvent.keyboard("{Enter}");
    fireEvent.keyDown(button, { key: "Enter" });
    expect(calls.filter((c) => c.method === "DELETE")).toHaveLength(0);
  });

  it("deletes with the token of the preview and the hash it expects, and tells the page", async () => {
    const onSettled = vi.fn();
    const onOpenChange = vi.fn();
    const { calls } = renderFile(<DeleteDialog {...props} onOpenChange={onOpenChange} onSettled={onSettled} />, { api: server() });
    await ready();
    await userEvent.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    await userEvent.click(screen.getByRole("button", { name: "Delete file" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith({ kind: "applied", hash: null, warnings: [] }));
    const del = calls.find((c) => c.method === "DELETE");
    const query = new URL(del?.path ?? "", "http://x");
    expect(query.pathname).toBe("/api/v1/swimlanes/sitb/files");
    expect(Object.fromEntries(query.searchParams)).toEqual({ path: props.path, expected: HASH_A, preview_id: "prev-del" });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("hands a conflict and a lock to the page, and keeps the dialog's own error for the rest", async () => {
    const onSettled = vi.fn();
    const { unmount } = renderFile(<DeleteDialog {...props} onSettled={onSettled} />, { api: server({ remove: () => problem(409, "conflict", { current: HASH_B }) }) });
    await ready();
    await userEvent.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    await userEvent.click(screen.getByRole("button", { name: "Delete file" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith(expect.objectContaining({ kind: "conflict" })));
    unmount();
    const again = vi.fn();
    renderFile(<DeleteDialog {...props} onSettled={again} />, { api: server({ remove: () => problem(422, "unprocessable", { reason: "not_allowed" }) }) });
    await ready();
    await userEvent.type(screen.getByLabelText(/Type .* to confirm/), NAME);
    await userEvent.click(screen.getByRole("button", { name: "Delete file" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not saved: the content was refused");
    expect(again).not.toHaveBeenCalled();
  });

  it("will not delete before the effect is known: a delete without a preview is refused by the server anyway", async () => {
    renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: (call) => (call.path.endsWith("/files/preview") ? new Promise<Response>(() => undefined) : json({})) });
    const field = await screen.findByLabelText(/Type .* to confirm/);
    await userEvent.type(field, NAME);
    expect(screen.getByRole("button", { name: "Delete file" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByLabelText("Working out what changes")).toBeInTheDocument();
  });

  it("says when the effect could not be worked out, and tries again on request", async () => {
    let fail = true;
    const { calls } = renderFile(<DeleteDialog {...props} onSettled={vi.fn()} />, { api: server({ preview: () => (fail ? problem(403, "forbidden") : json(PREVIEW)) }) });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await ready();
    expect(calls.filter((c) => c.path.endsWith("/files/preview"))).toHaveLength(2);
  });

  it("draws the file's name as data", async () => {
    renderFile(<DeleteDialog {...props} name={"a‮b.yml"} onSettled={vi.fn()} />, { api: server() });
    expect((await screen.findAllByText("<U+202E>")).length).toBeGreaterThan(0);
  });
});
