import { QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import type { FileEntry } from "@/api/generated/model";
import { createQueryClient } from "@/lib/query/client";
import { json, problem, stubApi } from "../test-utils";
import { useFileEntry } from "./useFileEntry";

const entry = (path: string, over: Partial<FileEntry> = {}): FileEntry => ({
  path,
  is_dir: false,
  kind: "base",
  class: "structured",
  role: "property_source",
  size: 10,
  hash: "a".repeat(64),
  drift: "nfs_ahead",
  format: null,
  content_withheld: false,
  attribution: null,
  severity: null,
  pickup_state: null,
  ...over,
});

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={createQueryClient()}>{children}</QueryClientProvider>;
}

describe("useFileEntry", () => {
  it("finds the file in the listing of its folder", async () => {
    const calls = stubApi((call) => {
      expect(call.path).toContain("/api/v1/swimlanes/sitb/tree?");
      expect(call.path).toContain("prefix=tx-infinity-api");
      return json({ items: [entry("tx-infinity-api/other.yml"), entry("tx-infinity-api/core-sit7.yml", { logical_file: "tx-infinity-api/core.yml", tenant: "sit7" })], next: null });
    });
    const { result } = renderHook(() => useFileEntry("sitb", "tx-infinity-api/core-sit7.yml"), { wrapper });
    expect(result.current.status).toBe("loading");
    await waitFor(() => expect(result.current.status).toBe("found"));
    expect(result.current.status === "found" && result.current.entry.tenant).toBe("sit7");
    expect(calls).toHaveLength(1);
  });

  it("asks for the root listing for a file at the root", async () => {
    const calls = stubApi(() => json({ items: [entry("root.yml")], next: null }));
    const { result } = renderHook(() => useFileEntry("sitb", "root.yml"), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("found"));
    expect(calls[0]?.path).not.toContain("prefix=");
  });

  it("reads on to the next page until it finds the file", async () => {
    let page = 0;
    stubApi(() => {
      page += 1;
      return page === 1 ? json({ items: [entry("d/a.yml")], next: "c2" }) : json({ items: [entry("d/b.yml")], next: null });
    });
    const { result } = renderHook(() => useFileEntry("sitb", "d/b.yml"), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("found"));
    expect(page).toBe(2);
  });

  it("says the file is missing when the folder is read to the end and it is not there", async () => {
    stubApi(() => json({ items: [entry("d/a.yml")], next: null }));
    const { result } = renderHook(() => useFileEntry("sitb", "d/zzz.yml"), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("missing"));
  });

  it("does not take a folder of the same name for the file", async () => {
    stubApi(() => json({ items: [entry("d/a", { is_dir: true })], next: null }));
    const { result } = renderHook(() => useFileEntry("sitb", "d/a"), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("missing"));
  });

  it("carries the problem when the listing cannot be read", async () => {
    stubApi(() => problem(403, "forbidden"));
    const { result } = renderHook(() => useFileEntry("sitb", "d/a.yml"), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.status === "error" && result.current.error.status).toBe(403);
  });
});
