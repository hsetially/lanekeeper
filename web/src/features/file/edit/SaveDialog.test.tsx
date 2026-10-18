import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { HASH_A, HASH_B, json, problem, renderFile } from "../test-utils";
import { ConvertEolDialog, SaveDialog, type SaveDialogProps } from "./SaveDialog";

// Save never happens silently (the keymap says so): it opens this review, which says where the text goes before it goes. The
// write carries the hash the edit was based on, the Idempotency-Key and the CSRF token (the mutator does), and its answer is
// the server's: applied, sent for approval, 409, 423 or a refusal.

const base: Omit<SaveDialogProps, "onSettled" | "onIssues"> = {
  open: true,
  onOpenChange: vi.fn(),
  lane: "sitb",
  path: "tx-infinity-api/tx-infinity-core-sit7.yml",
  name: "tx-infinity-core-sit7.yml",
  text: "a: 2\r\n",
  baseHash: HASH_A,
  rebasedFrom: null,
  approval: false,
  eol: "crlf",
};

const put = (body: unknown, status = 200) => (call: { method: string }) => (call.method === "PUT" ? json(body, status) : json({ items: [], next: null }));

describe("SaveDialog", () => {
  it("says where the text goes before it goes: straight to the NFS share, Git only by a PR", async () => {
    renderFile(<SaveDialog {...base} onSettled={vi.fn()} onIssues={vi.fn()} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    expect(within(dialog).getByText("Saving writes straight to the sitb NFS share. Git changes only through a PR.")).toBeInTheDocument();
    expect(within(dialog).getByText("aaaaaaa")).toBeInTheDocument();
    expect(within(dialog).getByText(/keeps its line endings \(CRLF\), encoding and BOM/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Save to sitb NFS" })).toBeInTheDocument();
  });

  it("starts with focus on Cancel", async () => {
    renderFile(<SaveDialog {...base} onSettled={vi.fn()} onIssues={vi.fn()} />);
    await screen.findByRole("alertdialog");
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());
  });

  it("says a write that needs approval is not written yet, and names the action Submit for approval", async () => {
    renderFile(<SaveDialog {...base} approval onSettled={vi.fn()} onIssues={vi.fn()} />);
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/needs an Admin to approve it before it is written to NFS/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Submit for approval" })).toBeInTheDocument();
  });

  it("sends the text with the hash it was based on, and the idempotency key", async () => {
    const { calls } = renderFile(<SaveDialog {...base} onSettled={vi.fn()} onIssues={vi.fn()} />, { api: put({ outcome: "applied", hash: HASH_B, audit_id: 9 }) });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PUT")).toBe(true));
    const sent = calls.find((c) => c.method === "PUT");
    expect(sent?.path).toBe("/api/v1/swimlanes/sitb/files");
    expect(JSON.parse(sent?.body ?? "{}")).toEqual({ path: base.path, expected: { state: "hash", hash: HASH_A }, content: "a: 2\r\n" });
    expect(sent?.headers.get("idempotency-key")).toBeTruthy();
  });

  it("expects the file to be absent when the edit is of a file that was gone", async () => {
    const { calls } = renderFile(<SaveDialog {...base} baseHash={null} onSettled={vi.fn()} onIssues={vi.fn()} />, { api: put({ outcome: "applied", hash: HASH_B, audit_id: 9 }) });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PUT")).toBe(true));
    expect((JSON.parse(calls.find((c) => c.method === "PUT")?.body ?? "{}") as { expected: unknown }).expected).toEqual({ state: "absent" });
  });

  it("says the save replaces a newer version when the person chose to re-apply on top of it", async () => {
    renderFile(<SaveDialog {...base} rebasedFrom={HASH_B} onSettled={vi.fn()} onIssues={vi.fn()} />);
    expect(await screen.findByText(/replaces the version that is on NFS now, which someone else saved/)).toBeInTheDocument();
  });

  it("hands the answer to the page and closes: applied", async () => {
    const onSettled = vi.fn();
    const onOpenChange = vi.fn();
    renderFile(<SaveDialog {...base} onOpenChange={onOpenChange} onSettled={onSettled} onIssues={vi.fn()} />, { api: put({ outcome: "applied", hash: HASH_B, audit_id: 9, warnings: [{ kind: "duplicate_key", line: 90 }] }) });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith({ kind: "applied", hash: HASH_B, warnings: [{ kind: "duplicate_key", line: 90 }] }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("hands the answer to the page: sent for approval, and a conflict", async () => {
    const onSettled = vi.fn();
    const { unmount } = renderFile(<SaveDialog {...base} onSettled={onSettled} onIssues={vi.fn()} />, { api: put({ outcome: "proposal_created", id: 2041 }) });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith({ kind: "sent", proposal: 2041 }));
    unmount();
    const again = vi.fn();
    renderFile(<SaveDialog {...base} onSettled={again} onIssues={vi.fn()} />, { api: (call) => (call.method === "PUT" ? problem(409, "conflict", { current: HASH_B }) : json({ items: [], next: null })) });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await waitFor(() => expect(again).toHaveBeenCalledWith(expect.objectContaining({ kind: "conflict", current: HASH_B })));
  });

  it("closes and marks the lines in the editor when the content was refused", async () => {
    const onIssues = vi.fn();
    const onOpenChange = vi.fn();
    renderFile(<SaveDialog {...base} onOpenChange={onOpenChange} onSettled={vi.fn()} onIssues={onIssues} />, {
      api: (call) => (call.method === "PUT" ? problem(422, "unprocessable", { reason: "secret_detected", issues: [{ kind: "secret_match", line: 120, rule: "private_key" }] }) : json({ items: [], next: null })),
    });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await waitFor(() => expect(onIssues).toHaveBeenCalledWith([{ kind: "secret_match", line: 120, rule: "private_key" }]));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("stays open with the error when the request failed, with the text intact", async () => {
    const onSettled = vi.fn();
    renderFile(<SaveDialog {...base} onSettled={onSettled} onIssues={vi.fn()} />, { api: (call) => (call.method === "PUT" ? problem(413, "payload_too_large") : json({ items: [], next: null })) });
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The file is too large");
    expect(alert).toHaveTextContent("Nothing was written");
    expect(onSettled).not.toHaveBeenCalled();
  });
});

describe("ConvertEolDialog", () => {
  const props = { open: true, onOpenChange: vi.fn(), lane: "sitb", path: base.path, name: base.name, text: "a: 1\r\n", baseHash: HASH_A, to: "lf" as const };

  it("says the text stays the same and only the line endings change", async () => {
    renderFile(<ConvertEolDialog {...props} onSettled={vi.fn()} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Convert tx-infinity-core-sit7.yml to LF?" });
    expect(within(dialog).getByText(/The text stays the same; only the line endings change/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Convert to LF" })).toBeInTheDocument();
  });

  it("sends the text unchanged with the new ending, based on the hash it read", async () => {
    const onSettled = vi.fn();
    const { calls } = renderFile(<ConvertEolDialog {...props} onSettled={onSettled} />, { api: put({ outcome: "applied", hash: HASH_B, audit_id: 3 }) });
    await userEvent.click(await screen.findByRole("button", { name: "Convert to LF" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalled());
    expect(JSON.parse(calls.find((c) => c.method === "PUT")?.body ?? "{}")).toEqual({ path: base.path, expected: { state: "hash", hash: HASH_A }, content: "a: 1\r\n", change_eol: "lf" });
  });
});
