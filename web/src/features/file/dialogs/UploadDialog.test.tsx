import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ChangePreview } from "@/api/generated/model";
import { HASH_A, HASH_B, json, problem, renderFile } from "../test-utils";
import { UploadDialog } from "./UploadDialog";

// Upload replaces the file with one from the person's disk. It is two steps that the server enforces: a preview (what would
// change, from the engine, with a token) and then the upload with that token. The file is never read by the page: it goes to
// the server as it is.

const PREVIEW: ChangePreview = {
  preview_id: "prev-1",
  expires_at: Date.UTC(2026, 9, 17, 10, 52),
  scope: { kind: "every_service" },
  warnings: [{ kind: "needs_notify_or_restart" }],
  changes: [{ change: { path: "tx.core.pool.maxSize", kind: "changed", left: { text: "40", ty: "int", flag: null }, right: { text: "25", ty: "int", flag: null }, left_location: null, right_location: null, severity: null }, flagged: false }],
  changes_total: 1,
  effective_changed: true,
};
const props = { open: true, onOpenChange: vi.fn(), lane: "sitb", path: "tx-infinity-api/tx-infinity-core-sit7.yml", name: "tx-infinity-core-sit7.yml", baseHash: HASH_A };
const file = () => new File(["tx:\n  core:\n    pool:\n      maxSize: 25\n"], "new.yml", { type: "text/yaml" });

const server = (over: { preview?: () => Response; upload?: () => Response } = {}) => (call: { method: string; path: string }) => {
  if (call.path.endsWith("/files/preview")) return over.preview?.() ?? json(PREVIEW);
  if (call.path.endsWith("/files/upload")) return over.upload?.() ?? json({ outcome: "applied", hash: HASH_B, audit_id: 5 });
  return json({ items: [], next: null });
};

describe("UploadDialog", () => {
  it("asks for a file first, and keeps the upload off until the effect is known", async () => {
    renderFile(<UploadDialog {...props} onSettled={vi.fn()} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Upload a file to replace tx-infinity-core-sit7.yml?" });
    expect(within(dialog).getByLabelText("File to upload")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Upload file" })).toHaveAttribute("aria-disabled", "true");
  });

  it("asks the engine what the file would change as soon as one is chosen, and shows it", async () => {
    const { calls } = renderFile(<UploadDialog {...props} onSettled={vi.fn()} />, { api: server() });
    await userEvent.upload(await screen.findByLabelText("File to upload"), file());
    expect(await screen.findByRole("table", { name: "What changes in the effective config" })).toBeInTheDocument();
    const sent = JSON.parse(calls.find((c) => c.path.endsWith("/files/preview"))?.body ?? "{}") as Record<string, string>;
    expect(sent).toMatchObject({ action: "upload", path: props.path, expected_state: "hash", expected_hash: HASH_A });
    expect(sent["file"]).toMatch(/^file:new\.yml:/);
    expect(screen.getByRole("button", { name: "Upload file" })).not.toHaveAttribute("aria-disabled", "true");
  });

  it("uploads with the token of the preview, and tells the page it was applied", async () => {
    const onSettled = vi.fn();
    const onOpenChange = vi.fn();
    const { calls } = renderFile(<UploadDialog {...props} onOpenChange={onOpenChange} onSettled={onSettled} />, { api: server() });
    await userEvent.upload(await screen.findByLabelText("File to upload"), file());
    await screen.findByRole("table");
    await userEvent.click(screen.getByRole("button", { name: "Upload file" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith({ kind: "applied", hash: HASH_B, warnings: [] }));
    const sent = JSON.parse(calls.find((c) => c.path.endsWith("/files/upload"))?.body ?? "{}") as Record<string, string>;
    expect(sent).toMatchObject({ path: props.path, expected_state: "hash", expected_hash: HASH_A, preview_id: "prev-1" });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("expects the file to be absent when there is none to replace", async () => {
    const { calls } = renderFile(<UploadDialog {...props} baseHash={null} onSettled={vi.fn()} />, { api: server() });
    await userEvent.upload(await screen.findByLabelText("File to upload"), file());
    await screen.findByRole("table");
    const sent = JSON.parse(calls.find((c) => c.path.endsWith("/files/preview"))?.body ?? "{}") as Record<string, string>;
    expect(sent["expected_state"]).toBe("absent");
    expect(sent).not.toHaveProperty("expected_hash");
  });

  it("does not send a file over 2 MiB: it says so", async () => {
    const { calls } = renderFile(<UploadDialog {...props} onSettled={vi.fn()} />, { api: server() });
    const big = new File([new Uint8Array(2 * 1024 * 1024 + 1)], "big.bin");
    await userEvent.upload(await screen.findByLabelText("File to upload"), big);
    expect(await screen.findByRole("alert")).toHaveTextContent("The file is larger than 2 MiB");
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(0);
  });

  it("shows the lines of a refused file and keeps the upload off", async () => {
    renderFile(<UploadDialog {...props} onSettled={vi.fn()} />, {
      api: server({ preview: () => problem(422, "unprocessable", { reason: "invalid_content", issues: [{ kind: "redaction_marker", line: 3 }] }) }),
    });
    await userEvent.upload(await screen.findByLabelText("File to upload"), file());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Line 3 holds a redaction marker");
    expect(screen.getByRole("button", { name: "Upload file" })).toHaveAttribute("aria-disabled", "true");
  });

  it("previews again when another file is chosen", async () => {
    const { calls } = renderFile(<UploadDialog {...props} onSettled={vi.fn()} />, { api: server() });
    const input = await screen.findByLabelText("File to upload");
    await userEvent.upload(input, file());
    await screen.findByRole("table");
    await userEvent.upload(input, new File(["other: 1\n"], "other.yml"));
    await waitFor(() => expect(calls.filter((c) => c.path.endsWith("/files/preview"))).toHaveLength(2));
  });

  it("hands a conflict to the page", async () => {
    const onSettled = vi.fn();
    renderFile(<UploadDialog {...props} onSettled={onSettled} />, { api: server({ upload: () => problem(409, "conflict", { current: HASH_B }) }) });
    await userEvent.upload(await screen.findByLabelText("File to upload"), file());
    await screen.findByRole("table");
    await userEvent.click(screen.getByRole("button", { name: "Upload file" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith(expect.objectContaining({ kind: "conflict" })));
  });
});
