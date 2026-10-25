import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderFile } from "../test-utils";
import { ConflictDialog, LockedDialog, SentDialog } from "./OutcomeDialogs";

// The three answers to a write that is not a plain success, in the words of the States index: 409 (File changed on NFS while
// you were editing), 423 (the lane is locked until it is adopted) and "Sent for approval" (not applied until an Admin decides).

const BASE = "a3f9c21" + "0".repeat(57);
const NOW = "b81e0d4" + "0".repeat(57);

describe("ConflictDialog", () => {
  const props = { open: true, onOpenChange: () => undefined, file: "tx-infinity-core-sit1.yml", lane: "sitb", yourBase: BASE, current: NOW, currentNote: "Tool write, 2 min ago", requestId: "req_8c41", onDiscard: () => undefined, onViewCurrent: () => undefined, onReapply: () => undefined };

  it("says nothing was applied, and names both versions", async () => {
    renderFile(<ConflictDialog {...props} />);
    const dialog = await screen.findByRole("dialog", { name: /File changed on NFS while you were editing/ });
    expect(within(dialog).getByText("Your save was not applied. Nothing on NFS changed.")).toBeInTheDocument();
    expect(within(dialog).getByText("a3f9c21")).toBeInTheDocument();
    expect(within(dialog).getByText("b81e0d4")).toBeInTheDocument();
    expect(within(dialog).getByText(/Tool write, 2 min ago/)).toBeInTheDocument();
    expect(within(dialog).getByText("tx-infinity-core-sit1.yml · sitb")).toBeInTheDocument();
  });

  it("shows the request id so a report can name the request", async () => {
    renderFile(<ConflictDialog {...props} />);
    expect(await screen.findByText("req_8c41")).toBeInTheDocument();
  });

  it("offers the three ways forward, and each one is a choice, never a default write", async () => {
    const onDiscard = vi.fn();
    const onViewCurrent = vi.fn();
    const onReapply = vi.fn();
    renderFile(<ConflictDialog {...props} onDiscard={onDiscard} onViewCurrent={onViewCurrent} onReapply={onReapply} />);
    await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button", { name: "View current" }));
    await userEvent.click(screen.getByRole("button", { name: "Discard my changes" }));
    await userEvent.click(screen.getByRole("button", { name: "Reload and re-apply" }));
    expect([onViewCurrent, onDiscard, onReapply].map((f) => f.mock.calls.length)).toEqual([1, 1, 1]);
  });

  it("says so when the file was removed, and re-applies as a new file", async () => {
    renderFile(<ConflictDialog {...props} current={null} currentNote={null} />);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/was removed from NFS/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "View current" })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Save as a new file" })).toBeInTheDocument();
  });

  it("has only Close when the write was not an edit, since there is nothing of the person's to re-apply", async () => {
    const onOpenChange = vi.fn();
    const { onDiscard: _a, onViewCurrent: _b, onReapply: _c, ...rest } = props;
    renderFile(<ConflictDialog {...rest} onOpenChange={onOpenChange} what="revert" />);
    const dialog = await screen.findByRole("dialog", { name: "The file changed on NFS first" });
    expect(within(dialog).getByText("Your revert was not applied. Nothing on NFS changed. Reload the file and try again.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Discard my changes" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Reload and re-apply" })).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("says the file was removed for a write that was not an edit", async () => {
    const { onDiscard: _a, onViewCurrent: _b, onReapply: _c, ...rest } = props;
    renderFile(<ConflictDialog {...rest} current={null} currentNote={null} what="delete" />);
    expect(await screen.findByText("Your delete was not applied. The file was removed from NFS first.")).toBeInTheDocument();
  });

  it("draws hostile text in a file name as data", async () => {
    renderFile(<ConflictDialog {...props} file={"a‮b.yml"} />);
    expect(await screen.findByText("<U+202E>")).toBeInTheDocument();
  });
});

describe("LockedDialog", () => {
  const base = { open: true, onOpenChange: () => undefined, action: "Save to presitc", lane: "presitc", baseline: "unconfirmed" as const, unknownFiles: 412, until: null, canAdopt: true, requestId: "req_2" };

  it("names the lane and says why: its baseline is unconfirmed", async () => {
    renderFile(<LockedDialog {...base} />);
    const dialog = await screen.findByRole("dialog", { name: /presitc is locked until it is adopted/ });
    expect(within(dialog).getByText(/Its baseline is unconfirmed, so edits are blocked/)).toBeInTheDocument();
    expect(within(dialog).getByText(/adopt 412 unknown files first/)).toBeInTheDocument();
    expect(within(dialog).getByText("Save to presitc")).toBeInTheDocument();
    expect(within(dialog).getByText("Unconfirmed")).toBeInTheDocument();
  });

  it("offers an Admin the way out: start adoption", async () => {
    renderFile(<LockedDialog {...base} />);
    const link = await screen.findByRole("link", { name: "Start adoption" });
    expect(link).toHaveAttribute("href", "/swimlanes/presitc/adopt");
  });

  it("tells anyone else to ask an Admin, and offers no way to start it", async () => {
    renderFile(<LockedDialog {...base} canAdopt={false} />);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("link", { name: "Start adoption" })).not.toBeInTheDocument();
    expect(within(dialog).getByText(/Ask an Admin/)).toBeInTheDocument();
  });

  it("says an adoption is open when it is", async () => {
    renderFile(<LockedDialog {...base} baseline="in_progress" unknownFiles={null} />);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Adoption in progress")).toBeInTheDocument();
  });

  it("does not guess a cause it was not told: another lock is just a lock, with when it ends", async () => {
    renderFile(<LockedDialog {...base} baseline="confirmed" unknownFiles={null} until={1_760_000_000_000} />);
    const dialog = await screen.findByRole("dialog", { name: /Editing is locked/ });
    expect(within(dialog).getByText(/Locked until 2025-10-09 08:53 UTC/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/baseline/i)).not.toBeInTheDocument();
  });
});

describe("SentDialog", () => {
  const base = { open: true, onOpenChange: () => undefined, file: "tx-infinity-core-sit11.yml", lane: "presita", proposal: 2041, canOpenApprovals: true };

  it("says it was not applied: nothing has changed on NFS yet", async () => {
    renderFile(<SentDialog {...base} />);
    const dialog = await screen.findByRole("dialog", { name: /Sent for approval/ });
    expect(within(dialog).getByText(/Nothing has changed on NFS yet/)).toBeInTheDocument();
    expect(within(dialog).getByText("AP-2041")).toBeInTheDocument();
    expect(within(dialog).getByText("tx-infinity-core-sit11.yml · presita")).toBeInTheDocument();
    expect(within(dialog).getByText("Pending approval")).toBeInTheDocument();
  });

  it("links an Admin to the request", async () => {
    renderFile(<SentDialog {...base} />);
    expect(await screen.findByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/2041");
  });

  it("does not link someone who cannot open Approvals, and says who decides", async () => {
    renderFile(<SentDialog {...base} canOpenApprovals={false} />);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("link", { name: "View request" })).not.toBeInTheDocument();
    expect(within(dialog).getByText(/Any Admin/)).toBeInTheDocument();
  });

  it("closes with Stay on file", async () => {
    const onOpenChange = vi.fn();
    renderFile(<SentDialog {...base} onOpenChange={onOpenChange} />);
    await userEvent.click(await screen.findByRole("button", { name: "Stay on file" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
