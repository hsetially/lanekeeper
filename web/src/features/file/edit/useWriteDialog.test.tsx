import { QueryClientProvider, useMutation, useQueryClient } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/lib/query/client";
import { fileMutations } from "../mutations";
import { HASH_A, HASH_B, json, problem, stubApi } from "../test-utils";
import type { WriteOutcome } from "./outcome";
import { useWriteDialog } from "./useWriteDialog";

// The state every write dialog shares: it is working while the request is out, an answer that is a dialog of its own (applied,
// sent for approval, 409, 423) goes to the screen, and one that stays here (refused, too large, failed) is the dialog's
// inline error with the person's input intact. A write is never optimistic and never retried.

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={createQueryClient()}>{children}</QueryClientProvider>;
}

const body = { swimlane: "sitb", body: { path: "a.yml", expected: { state: "hash" as const, hash: HASH_A }, content: "x: 1\n" } };

function setup(onSettled: (outcome: WriteOutcome) => void) {
  return renderHook(
    () => {
      const client = useQueryClient();
      const mutation = useMutation(fileMutations.edit(client));
      return useWriteDialog(mutation, onSettled);
    },
    { wrapper },
  );
}

describe("useWriteDialog", () => {
  it("sends the write with the expected hash, and tells the screen it was applied", async () => {
    const calls = stubApi((call) => (call.method === "PUT" ? json({ outcome: "applied", hash: HASH_B, audit_id: 1 }) : json({ items: [], next: null })));
    const onSettled = vi.fn();
    const { result } = setup(onSettled);
    await act(() => result.current.run(body));
    expect(onSettled).toHaveBeenCalledWith({ kind: "applied", hash: HASH_B, warnings: [] });
    const put = calls.find((c) => c.method === "PUT");
    expect(put?.path).toBe("/api/v1/swimlanes/sitb/files");
    expect(JSON.parse(put?.body ?? "{}")).toMatchObject({ path: "a.yml", expected: { state: "hash", hash: HASH_A } });
    expect(put?.headers.get("idempotency-key")).toMatch(/\S+/);
    expect(put?.headers.get("x-csrf-token")).toMatch(/\S+/);
    expect(result.current.error).toBeNull();
  });

  it("tells the screen a write was only sent for approval", async () => {
    stubApi((call) => (call.method === "PUT" ? json({ outcome: "proposal_created", id: 2041 }) : json({ items: [], next: null })));
    const onSettled = vi.fn();
    const { result } = setup(onSettled);
    await act(() => result.current.run(body));
    expect(onSettled).toHaveBeenCalledWith({ kind: "sent", proposal: 2041 });
  });

  it("hands a conflict to the screen, with the hash that is on NFS now", async () => {
    stubApi((call) => (call.method === "PUT" ? problem(409, "conflict", { current: HASH_B }) : json({ items: [], next: null })));
    const onSettled = vi.fn();
    const { result } = setup(onSettled);
    await act(() => result.current.run(body));
    expect(onSettled).toHaveBeenCalledWith({ kind: "conflict", current: HASH_B, requestId: "req-test-1" });
    expect(result.current.error).toBeNull();
  });

  it("hands a lock to the screen", async () => {
    stubApi((call) => (call.method === "PUT" ? problem(423, "locked") : json({ items: [], next: null })));
    const onSettled = vi.fn();
    const { result } = setup(onSettled);
    await act(() => result.current.run(body));
    expect(onSettled).toHaveBeenCalledWith(expect.objectContaining({ kind: "locked" }));
  });

  it("keeps a refusal in the dialog, with the issues for the editor to mark", async () => {
    stubApi((call) => (call.method === "PUT" ? problem(422, "unprocessable", { reason: "secret_detected", issues: [{ kind: "secret_match", line: 120, rule: "private_key" }] }) : json({ items: [], next: null })));
    const onSettled = vi.fn();
    const { result } = setup(onSettled);
    await act(() => result.current.run(body));
    expect(onSettled).not.toHaveBeenCalled();
    await waitFor(() => expect(result.current.error?.title).toBe("Not saved: the content was refused"));
    expect(result.current.issues).toEqual([{ kind: "secret_match", line: 120, rule: "private_key" }]);
  });

  it("keeps a failed request in the dialog and does not retry it", async () => {
    const calls = stubApi((call) => (call.method === "PUT" ? problem(500, "internal") : json({ items: [], next: null })));
    const onSettled = vi.fn();
    const { result } = setup(onSettled);
    await act(() => result.current.run(body));
    await waitFor(() => expect(result.current.error?.title).toBe("internal"));
    expect(calls.filter((c) => c.method === "PUT")).toHaveLength(1);
    expect(onSettled).not.toHaveBeenCalled();
  });

  it("clears the error on the next attempt, and on request", async () => {
    let fail = true;
    stubApi((call) => (call.method === "PUT" ? (fail ? problem(500, "internal") : json({ outcome: "applied", hash: HASH_B, audit_id: 1 })) : json({ items: [], next: null })));
    const { result } = setup(vi.fn());
    await act(() => result.current.run(body));
    await waitFor(() => expect(result.current.error).not.toBeNull());
    fail = false;
    await act(() => result.current.run(body));
    expect(result.current.error).toBeNull();
    fail = true;
    await act(() => result.current.run(body));
    await waitFor(() => expect(result.current.error).not.toBeNull());
    act(() => result.current.clear());
    expect(result.current.error).toBeNull();
    expect(result.current.issues).toEqual([]);
  });
});
