import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/lib/query/client";
import { json, stubHub } from "../test-utils";
import { FilePicker } from "./FilePicker";

const ENTRIES = [
  { file: "tx-infinity-api/tx-infinity-core.yml", state: "different" },
  { file: "tx-infinity-api/tx-infinity-ledger.yml", state: "left_only" },
  { file: "resources/receipt-footer.txt", state: "right_only" },
  { file: "security/same.yml", state: "same" },
];

function view(over: Partial<Parameters<typeof FilePicker>[0]> = {}) {
  const onChoose = vi.fn();
  function Host() {
    const [open, setOpen] = useState(true);
    return (
      <QueryClientProvider client={createQueryClient()}>
        <FilePicker file={null} sides={["sit", "sitb"]} open={open} onOpenChange={setOpen} onChoose={onChoose} {...over} />
      </QueryClientProvider>
    );
  }
  render(<Host />);
  return { onChoose };
}

describe("the file picker", () => {
  it("says what is chosen, or that nothing is", () => {
    stubHub(() => json({ entries: { items: [], next: null }, skipped_identical_subtrees: 0 }));
    view({ open: false, onOpenChange: () => undefined, file: "a/b.yml" });
    expect(screen.getByRole("button", { name: /Logical file: a\/b\.yml/ })).toBeInTheDocument();
  });

  it("lists the files that differ between the first two swimlanes, and not the ones that are the same", async () => {
    const calls = stubHub(() => json({ entries: { items: ENTRIES, next: null }, skipped_identical_subtrees: 4 }));
    view();
    const list = await screen.findByRole("list", { name: "Files that differ" });
    expect(within(list).getAllByRole("button")).toHaveLength(3);
    expect(within(list).queryByText(/same\.yml/)).toBeNull();
    const url = calls[0]?.url;
    expect(url?.pathname).toBe("/api/v1/compare/tree");
    expect(url?.searchParams.get("left")).toBe("effective:sit");
    expect(url?.searchParams.get("right")).toBe("effective:sitb");
  });

  it("chooses a file from the list", async () => {
    const user = userEvent.setup();
    stubHub(() => json({ entries: { items: ENTRIES, next: null }, skipped_identical_subtrees: 0 }));
    const { onChoose } = view();
    await user.click(await screen.findByRole("button", { name: /tx-infinity-ledger/ }));
    expect(onChoose).toHaveBeenCalledWith("tx-infinity-api/tx-infinity-ledger.yml");
  });

  it("narrows the list as the person types", async () => {
    const user = userEvent.setup();
    stubHub(() => json({ entries: { items: ENTRIES, next: null }, skipped_identical_subtrees: 0 }));
    view();
    await screen.findByRole("list", { name: "Files that differ" });
    await user.type(screen.getByRole("textbox", { name: "Logical file" }), "RECEIPT");
    await waitFor(() => expect(within(screen.getByRole("list", { name: "Files that differ" })).getAllByRole("button")).toHaveLength(1));
  });

  it("takes a path that is typed or pasted, with Enter", async () => {
    const user = userEvent.setup();
    stubHub(() => json({ entries: { items: [], next: null }, skipped_identical_subtrees: 0 }));
    const { onChoose } = view();
    await user.type(screen.getByRole("textbox", { name: "Logical file" }), "ops/batch/nightly.yml{Enter}");
    expect(onChoose).toHaveBeenCalledWith("ops/batch/nightly.yml");
  });

  it("refuses a path the server would refuse, and says why", async () => {
    const user = userEvent.setup();
    stubHub(() => json({ entries: { items: [], next: null }, skipped_identical_subtrees: 0 }));
    const { onChoose } = view();
    await user.type(screen.getByRole("textbox", { name: "Logical file" }), "../etc/passwd{Enter}");
    expect(onChoose).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "Logical file" })).toBeInvalid();
    expect(screen.getByText(/path below the config root/i)).toBeInTheDocument();
  });

  it("asks nothing and says so when fewer than two swimlanes are chosen", () => {
    const calls = stubHub(() => json({}));
    view({ sides: ["sit"] });
    expect(calls).toHaveLength(0);
    expect(screen.getByText(/Choose two swimlanes to list the files that differ/)).toBeInTheDocument();
  });

  it("says when the list cannot be read, with the request id, and still takes a typed path", async () => {
    stubHub(() => new Response(JSON.stringify({ type: "urn:lanekeeper:problem:internal", title: "Internal error", status: 500, request_id: "req-9" }), { status: 500, headers: { "content-type": "application/problem+json" } }));
    view();
    expect(await screen.findByText(/Couldn't load files that differ/, undefined, { timeout: 8000 })).toBeInTheDocument();
    expect(screen.getByText("req-9")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Logical file" })).toBeEnabled();
  }, 15000);
});
