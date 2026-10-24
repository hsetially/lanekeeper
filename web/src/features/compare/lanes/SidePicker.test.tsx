import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SwimlaneSummary } from "@/api/generated/model";
import { createQueryClient } from "@/lib/query/client";
import { json, stubHub } from "../test-utils";
import { SidePicker } from "./SidePicker";

const lane = (id: string): SwimlaneSummary => ({ id, display_name: id, tenants: [`t-${id}`], agent: "connected", last_scan_at: 0, base_version: null, drift_counts: [], finding_count: 0, tier: "SIT" });

function show(target: "lane" | "branch" | "tag", value: string | null = null) {
  const onChoose = vi.fn();
  render(
    <QueryClientProvider client={createQueryClient()}>
      <SidePicker side="Right" target={target} value={value} lanes={[lane("sitb"), lane("sitc")]} onChoose={onChoose} />
    </QueryClientProvider>,
  );
  return { onChoose };
}

describe("a side of the comparison", () => {
  it("says what is chosen, or what to choose", () => {
    stubHub(() => json({ items: [], next: null }));
    show("tag");
    expect(screen.getByRole("button", { name: "Right side: choose a Git tag" })).toBeInTheDocument();
  });

  it("chooses a swimlane from the list, filtered by what is typed", async () => {
    const user = userEvent.setup();
    stubHub(() => json({}));
    const { onChoose } = show("lane", "sitb");
    await user.click(screen.getByRole("button", { name: "Right side: sitb" }));
    await user.type(screen.getByRole("textbox", { name: "Filter swimlanes" }), "sitc");
    await user.click(screen.getByRole("button", { name: /sitc/ }));
    expect(onChoose).toHaveBeenCalledWith("sitc");
  });

  it("asks the server for the branches of the tenant repository, by the start of the name typed", async () => {
    const user = userEvent.setup();
    const calls = stubHub(() => json({ items: [{ name: "sit7", kind: "branch", commit: "a".repeat(40), branch_kind: "deployable" }], next: null }));
    const { onChoose } = show("branch");
    await user.click(screen.getByRole("button", { name: /Right side/ }));
    await user.click(await screen.findByRole("button", { name: /sit7/ }));
    expect(onChoose).toHaveBeenCalledWith("sit7");
    expect(calls[0]?.url.searchParams.get("repo")).toBe("tenant");
    expect(calls[0]?.url.searchParams.get("kind")).toBe("branch");
    expect(calls[0]?.url.searchParams.has("q")).toBe(false);
  });

  it("asks for the tags of the base repository", async () => {
    const user = userEvent.setup();
    const calls = stubHub(() => json({ items: [{ name: "v2.3.1", kind: "tag", commit: "b".repeat(40), branch_kind: null }], next: null }));
    show("tag");
    await user.click(screen.getByRole("button", { name: /Right side/ }));
    await screen.findByRole("button", { name: /v2\.3\.1/ });
    expect(calls[0]?.url.searchParams.get("repo")).toBe("base");
    expect(calls[0]?.url.searchParams.get("kind")).toBe("tag");
    await user.type(screen.getByRole("textbox", { name: "Filter Git tags" }), "v2");
    await waitFor(() => expect(calls.some((c) => c.url.searchParams.get("q") === "v2")).toBe(true));
  });

  it("says when no ref starts with what was typed", async () => {
    const user = userEvent.setup();
    stubHub(() => json({ items: [], next: null }));
    show("branch");
    await user.click(screen.getByRole("button", { name: /Right side/ }));
    expect(await screen.findByText(/has no branches/)).toBeInTheDocument();
  });
});
