import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Version } from "@/api/generated/model";
import { HASH_A, HASH_B, json, problem, renderFile } from "../test-utils";
import { RevertDialog } from "./RevertDialog";

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const version: Version = {
  hash: "c19a4e2" + "0".repeat(57),
  observed_at: Date.UTC(2026, 9, 17, 13, 58),
  source: "tool_write",
  size: 120,
  attribution: { source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME }, evidence: {} },
  severity: "low",
  commit: null,
};
const props = { open: true, onOpenChange: vi.fn(), id: "sitb", path: "tx-infinity-api/tx-infinity-core-sit7.yml", version, currentHash: HASH_A, onSettled: vi.fn() };

describe("RevertDialog", () => {
  it("names the version and says what it does, consequence first (1k)", async () => {
    renderFile(<RevertDialog {...props} />);
    const dialog = await screen.findByRole("alertdialog", { name: "Revert to c19a4e2?" });
    expect(within(dialog).getByText(/replaces the file on NFS with the content of this version/i)).toBeInTheDocument();
    // The author is untrusted text and goes through SafeText, so it is an element of its own inside the sentence.
    expect(within(dialog).getByText("You").closest("dd")).toHaveTextContent("Tool write by You, 2026-10-17 13:58 UTC");
    expect(within(dialog).getByText(/Nothing is deleted from history\./)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Revert to c19a4e2" })).toBeInTheDocument();
  });

  it("puts focus on the safe choice, not on the revert", async () => {
    renderFile(<RevertDialog {...props} />);
    await screen.findByRole("alertdialog");
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());
  });

  it("sends the revert with the hash that is on NFS now, and the version to restore", async () => {
    const onSettled = vi.fn();
    const onOpenChange = vi.fn();
    const { calls } = renderFile(<RevertDialog {...props} onSettled={onSettled} onOpenChange={onOpenChange} />, {
      api: (call) => (call.method === "POST" ? json({ outcome: "applied", hash: HASH_B, audit_id: 4 }) : json({ items: [], next: null })),
    });
    await userEvent.click(await screen.findByRole("button", { name: "Revert to c19a4e2" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith({ kind: "applied", hash: HASH_B, warnings: [] }));
    const post = calls.find((c) => c.method === "POST");
    expect(post?.path).toBe("/api/v1/swimlanes/sitb/files/revert");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ path: "tx-infinity-api/tx-infinity-core-sit7.yml", expected: { state: "hash", hash: HASH_A }, to: version.hash });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("expects the file to be absent when there is nothing on NFS to revert over", async () => {
    const { calls } = renderFile(<RevertDialog {...props} currentHash={null} />, { api: (call) => (call.method === "POST" ? json({ outcome: "applied", hash: HASH_B, audit_id: 4 }) : json({ items: [], next: null })) });
    await userEvent.click(await screen.findByRole("button", { name: "Revert to c19a4e2" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect((JSON.parse(calls.find((c) => c.method === "POST")?.body ?? "{}") as { expected: unknown }).expected).toEqual({ state: "absent" });
  });

  it("hands a conflict, a lock and a proposal to the screen, and closes", async () => {
    const onSettled = vi.fn();
    const onOpenChange = vi.fn();
    renderFile(<RevertDialog {...props} onSettled={onSettled} onOpenChange={onOpenChange} />, { api: (call) => (call.method === "POST" ? problem(409, "conflict", { current: HASH_B }) : json({ items: [], next: null })) });
    await userEvent.click(await screen.findByRole("button", { name: "Revert to c19a4e2" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith(expect.objectContaining({ kind: "conflict", current: HASH_B })));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("stays open with the error when the revert fails, and says nothing was written", async () => {
    const onSettled = vi.fn();
    renderFile(<RevertDialog {...props} onSettled={onSettled} />, { api: (call) => (call.method === "POST" ? problem(422, "unprocessable", { reason: "not_allowed" }) : json({ items: [], next: null })) });
    await userEvent.click(await screen.findByRole("button", { name: "Revert to c19a4e2" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Not saved: the content was refused");
    expect(alert).toHaveTextContent("req-test-1");
    expect(onSettled).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });

  it("renders nothing when there is no version to revert to", () => {
    renderFile(<RevertDialog {...props} version={null} />);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });
});
