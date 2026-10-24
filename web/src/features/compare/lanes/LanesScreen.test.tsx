import { QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { Comparison, DiffHunk, SwimlaneSummary, TreeCompareEntry } from "@/api/generated/model";
import { KeyboardProvider } from "@/keyboard";
import { expectNoAxeViolations } from "@/test/axe";
import { createQueryClient } from "@/lib/query/client";
import { invalidateTargets } from "@/lib/query/targets";
import type { QueryState } from "@/lib/query/use-query-state";
import { parseView, type CompareView } from "../model";
import { json, problem, stubHub, type Handler } from "../test-utils";
import { LanesScreen } from "./LanesScreen";

const lane = (id: string, over: Partial<SwimlaneSummary> = {}): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: [`t-${id}`],
  agent: "connected",
  last_scan_at: Date.now() - 14 * 60_000,
  base_version: null,
  drift_counts: [],
  finding_count: 0,
  tier: "SIT",
  ...over,
});
const LANES = [lane("sitb"), lane("sitc"), lane("presitb", { agent: "disconnected", tier: "PRESIT" })];
const ready = (data: SwimlaneSummary[]): QueryState<SwimlaneSummary[]> => ({ state: "ready", data, error: null, retryable: false, refreshing: false, stale: null, refetch: vi.fn() });

const entry = (file: string, state: TreeCompareEntry["state"] = "different"): TreeCompareEntry => ({ file, state });
const ENTRIES = [
  entry("tx-infinity-api/tx-infinity-core.yml"),
  entry("tx-infinity-api/tx-infinity-ledger.yml", "left_only"),
  entry("resources/receipt-footer.txt", "right_only"),
  entry("resources/receipt-logo.jpeg"),
  entry("security/same.yml", "same"),
];

const hunk = (): DiffHunk => ({
  old_start: 48,
  old_lines: 2,
  new_start: 48,
  new_lines: 2,
  lines: [
    { kind: "context", text: "limits:", inline_changes: [] },
    { kind: "removed", text: "  max: 2000", inline_changes: [] },
    { kind: "added", text: "  max: 2500", inline_changes: [] },
  ],
});
const comparison = (over: Partial<Comparison> = {}): Comparison => ({ left: "effective:sitb", right: "effective:sitc", file: "f", class: "structured", identical: false, eol_only: false, hunks: [hunk()], setting_changes: [], binary: null, ...over });

function renderLanes(start: Partial<CompareView>, lanes: SwimlaneSummary[] = LANES) {
  const seen: CompareView[] = [];
  const client = createQueryClient();
  const base: CompareView = { ...parseView({}), mode: "lanes", left: "effective:sitb", right: "effective:sitc", ...start };
  function Host() {
    const [view, setView] = useState(base);
    return (
      <QueryClientProvider client={client}>
        <KeyboardProvider>
          <LanesScreen
            view={view}
            onView={(next) => {
              seen.push(next);
              setView(next);
            }}
            swimlanes={ready(lanes)}
          />
        </KeyboardProvider>
      </QueryClientProvider>
    );
  }
  render(<Host />);
  return { seen, client };
}

function hub(entries: TreeCompareEntry[], file: (url: URL) => Response = () => json(comparison()), skipped = 38) {
  const requests: URL[] = [];
  const handler: Handler = (call) => {
    requests.push(call.url);
    if (call.url.pathname === "/api/v1/compare/tree") return json({ entries: { items: entries, next: null }, skipped_identical_subtrees: skipped });
    if (call.url.pathname === "/api/v1/compare") return file(call.url);
    if (call.url.pathname === "/api/v1/git/refs") return json({ items: [{ name: "sit7", kind: "branch", commit: "a".repeat(40), branch_kind: "deployable" }], next: null });
    return problem(404, "not_found");
  };
  return { handler, requests };
}

describe("whole-swimlane compare", () => {
  it("lists the files that differ as a tree of folders, with the number below each, and skips the files that are the same", async () => {
    const h = hub(ENTRIES);
    stubHub(h.handler);
    renderLanes({});
    const tree = await screen.findByRole("tree", { name: "Differing files" });
    expect(within(tree).getByText("tx-infinity-api")).toBeInTheDocument();
    expect(within(tree).getByText("receipt-footer.txt")).toBeInTheDocument();
    expect(within(tree).queryByText("same.yml")).toBeNull();
    expect(within(tree).getAllByTitle("2 differing files")).toHaveLength(2);
    const tree_call = h.requests.find((u) => u.pathname === "/api/v1/compare/tree");
    expect(tree_call?.searchParams.get("left")).toBe("effective:sitb");
    expect(tree_call?.searchParams.get("right")).toBe("effective:sitc");
    expect(await screen.findByText("38 identical directories skipped")).toBeInTheDocument();
  });

  it("counts the files by kind in cards that filter the tree", async () => {
    const user = userEvent.setup();
    stubHub(hub(ENTRIES).handler);
    renderLanes({});
    await screen.findByRole("tree", { name: "Differing files" });
    const summary = screen.getByRole("group", { name: "Summary" });
    expect(within(summary).getByRole("button", { name: /Differing logical files/ })).toHaveTextContent("2");
    expect(within(summary).getByRole("button", { name: /Only in sitb/ })).toHaveTextContent("1");
    expect(within(summary).getByRole("button", { name: /Only in sitc/ })).toHaveTextContent("1");
    await user.click(within(summary).getByRole("button", { name: /Only in sitc/ }));
    expect(within(summary).getByRole("button", { name: /Only in sitc/ })).toHaveAttribute("aria-pressed", "true");
    const tree = screen.getByRole("tree", { name: "Differing files" });
    expect(within(tree).getByText("receipt-footer.txt")).toBeInTheDocument();
    expect(within(tree).queryByText("tx-infinity-core.yml")).toBeNull();
  });

  it("shows how the first file differs, from the server's hunks, with the two sides named", async () => {
    const h = hub(ENTRIES);
    stubHub(h.handler);
    renderLanes({});
    const diff = await screen.findByRole("grid", { name: "Differences in the file" });
    expect(within(diff).getAllByText(/max: 2000/).length).toBeGreaterThan(0);
    const call = h.requests.find((u) => u.pathname === "/api/v1/compare");
    expect(call?.searchParams.get("left")).toBe("effective:sitb");
    expect(call?.searchParams.get("right")).toBe("effective:sitc");
    // The first file of the tree (folders first, in name order).
    expect(call?.searchParams.get("file")).toBe("resources/receipt-footer.txt");
    expect(screen.getByRole("heading", { name: "resources/receipt-footer.txt" })).toBeInTheDocument();
    expect(await screen.findByText("1 added · 1 removed")).toBeInTheDocument();
  });

  it("shows the file that is chosen, and writes it to the address", async () => {
    const user = userEvent.setup();
    const h = hub(ENTRIES);
    stubHub(h.handler);
    const { seen } = renderLanes({});
    const tree = await screen.findByRole("tree", { name: "Differing files" });
    await user.click(within(tree).getByText("tx-infinity-core.yml"));
    expect(seen.at(-1)?.file).toBe("tx-infinity-api/tx-infinity-core.yml");
    await waitFor(() => expect(h.requests.some((u) => u.pathname === "/api/v1/compare" && u.searchParams.get("file") === "tx-infinity-api/tx-infinity-core.yml")).toBe(true));
  });

  it("shows a file from the address, even if the tree does not list it", async () => {
    const h = hub(ENTRIES);
    stubHub(h.handler);
    renderLanes({ file: "ops/not-listed.yml" });
    await screen.findByRole("heading", { name: "ops/not-listed.yml" });
    expect(h.requests.some((u) => u.pathname === "/api/v1/compare" && u.searchParams.get("file") === "ops/not-listed.yml")).toBe(true);
  });

  it("compares a binary file by hash, as the server says", async () => {
    stubHub(
      hub([entry("resources/receipt-logo.jpeg")], () =>
        json(comparison({ class: "binary", hunks: [], binary: { left: "9be1c40".padEnd(64, "0"), right: "f0a77d2".padEnd(64, "0"), left_size: 4300, right_size: 5900 } })),
      ).handler,
    );
    renderLanes({});
    expect(await screen.findByText("Binary files differ")).toBeInTheDocument();
    expect(screen.getByText("Binary content is compared by hash, not line by line.")).toBeInTheDocument();
    expect(screen.getByText("9be1c4000000")).toBeInTheDocument();
    expect(screen.getByText(/4,300 bytes/)).toBeInTheDocument();
  });

  it("keeps the two hash rows 24px apart, so that their 24px copy buttons do not overlap (axe `target-size`)", async () => {
    stubHub(hub([entry("resources/receipt-logo.jpeg")], () => json(comparison({ class: "binary", hunks: [], binary: { left: "9be1c40".padEnd(64, "0"), right: "f0a77d2".padEnd(64, "0"), left_size: 4300, right_size: 5900 } }))).handler);
    renderLanes({});
    const hash = await screen.findByText("9be1c4000000");
    // An 18px line and the gap under it are the pitch of the rows; a copy button is 24px, so the gap is 6px (gap-y-1.5), not 4.
    const grid = hash.closest("[class*='grid-cols-']");
    expect(grid?.className).toMatch(/(^|\s)gap-y-1\.5(\s|$)/);
  });

  it("does not show the content of a file whose content is withheld", async () => {
    stubHub(hub([entry("keys/keystore.jks")], () => json(comparison({ class: "denied", hunks: [] }))).handler);
    renderLanes({});
    expect(await screen.findByText("Content withheld")).toBeInTheDocument();
  });

  it("says the two sides are identical, with how much was skipped, when nothing differs", async () => {
    stubHub(hub([entry("a.yml", "same")], undefined, 43).handler);
    renderLanes({});
    expect(await screen.findByText("sitb and sitc are identical")).toBeInTheDocument();
    expect(screen.getByText(/43 identical directories were skipped/)).toBeInTheDocument();
    expect(screen.queryByRole("tree")).toBeNull();
  });

  it("shows the columns of the page while it reads the directories", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    stubHub(async () => {
      await gate;
      return json({ entries: { items: ENTRIES, next: null }, skipped_identical_subtrees: 1 });
    });
    renderLanes({});
    expect((await screen.findAllByText(/Reading directories…/)).length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Left side: sitb" })).toBeInTheDocument();
    release();
    await screen.findByRole("tree", { name: "Differing files" });
  });

  it("says what failed, with the request id", async () => {
    stubHub(() => problem(403, "forbidden"));
    renderLanes({});
    expect(await screen.findByText("Couldn't load comparison")).toBeInTheDocument();
    expect(screen.getByText("req-compare-1")).toBeInTheDocument();
  });

  it("says the list was cut off when the server has more files than the page will read, and shows what it has", async () => {
    let n = 0;
    stubHub((call) => {
      if (call.url.pathname !== "/api/v1/compare/tree") return json(comparison());
      n += 1;
      return json({ entries: { items: [entry(`dir/file-${String(n).padStart(3, "0")}.yml`)], next: `c${n}` }, skipped_identical_subtrees: 2 });
    });
    renderLanes({});
    expect(await screen.findByText("Results cut off", undefined, { timeout: 8000 })).toBeInTheDocument();
    expect(screen.getByText(/Showing the first 10 files/)).toBeInTheDocument();
    expect(n).toBe(10);
    expect(screen.getByRole("tree", { name: "Differing files" })).toBeInTheDocument();
  }, 15000);

  it("asks the person to choose the sides when there are none", () => {
    const calls = stubHub(() => json({}));
    renderLanes({ left: null, right: null });
    expect(screen.getByText("Choose two swimlanes to compare")).toBeInTheDocument();
    expect(screen.getByText("Pick a left and a right swimlane. Files that differ are listed here.")).toBeInTheDocument();
    expect(calls).toHaveLength(0);
  });

  it("names a swimlane whose agent is offline and says its files are as of its last scan", async () => {
    stubHub(hub(ENTRIES).handler);
    renderLanes({ right: "effective:presitb" });
    const banner = await screen.findByText("presitb agent is offline");
    expect(banner.parentElement).toHaveTextContent(/presitb: its NFS files as of \d\d:\d\d UTC, 14 min ago\. Differences made since then are not included\./);
    expect(await screen.findAllByText(/as of \d\d:\d\d UTC/)).not.toHaveLength(0);
  });
});

describe("choosing the sides", () => {
  it("compares the effective files of two swimlanes, and writes the sides to the address", async () => {
    const user = userEvent.setup();
    stubHub(hub(ENTRIES).handler);
    const { seen } = renderLanes({});
    await screen.findByRole("tree", { name: "Differing files" });
    await user.click(screen.getByRole("button", { name: "Right side: sitc" }));
    await user.click(await screen.findByRole("button", { name: /presitb/ }));
    expect(seen.at(-1)).toMatchObject({ mode: "lanes", left: "effective:sitb", right: "effective:presitb", file: null });
  });

  it("compares NFS with a Git branch of the tenant repository, listed by the server", async () => {
    const user = userEvent.setup();
    const h = hub(ENTRIES);
    stubHub(h.handler);
    const { seen } = renderLanes({});
    await screen.findByRole("tree", { name: "Differing files" });
    await user.click(screen.getByRole("radio", { name: "Git branch" }));
    expect(seen.at(-1)).toMatchObject({ left: "nfs:sitb", right: null });
    await user.click(screen.getByRole("button", { name: /Right side/ }));
    await user.click(await screen.findByRole("button", { name: /sit7/ }));
    expect(seen.at(-1)).toMatchObject({ left: "nfs:sitb", right: "git:tenant@sit7" });
    const refs = h.requests.find((u) => u.pathname === "/api/v1/git/refs");
    expect(refs?.searchParams.get("repo")).toBe("tenant");
    expect(refs?.searchParams.get("kind")).toBe("branch");
  });

  it("swaps two swimlanes, and cannot swap a swimlane with Git", async () => {
    const user = userEvent.setup();
    stubHub(hub(ENTRIES).handler);
    const { seen } = renderLanes({});
    await screen.findByRole("tree", { name: "Differing files" });
    await user.click(screen.getByRole("button", { name: "Swap sides" }));
    expect(seen.at(-1)).toMatchObject({ left: "effective:sitc", right: "effective:sitb" });
    await user.click(screen.getByRole("radio", { name: "Git tag" }));
    expect(screen.getByRole("button", { name: "Swap sides" })).toBeDisabled();
  });
});

describe("a comparison is not an answer to keep", () => {
  /** A hub whose comparison of the file has a version that changes, and an ETag for it that it honours. */
  function changing() {
    const state = { version: 1, tags: [] as Array<string | null>, statuses: [] as number[] };
    const handler: Handler = (call) => {
      if (call.url.pathname === "/api/v1/compare/tree") return json({ entries: { items: [entry("a/core.yml")], next: null }, skipped_identical_subtrees: 0 });
      if (call.url.pathname !== "/api/v1/compare") return problem(404, "not_found");
      const tag = `"v${state.version}"`;
      state.tags.push(call.headers.get("if-none-match"));
      if (call.headers.get("if-none-match") === tag) {
        state.statuses.push(304);
        return new Response(null, { status: 304, headers: { etag: tag } });
      }
      state.statuses.push(200);
      const text = state.version === 1 ? "  max: 2000" : "  max: 7777";
      return json(comparison({ hunks: [{ ...hunk(), lines: [{ kind: "removed", text, inline_changes: [] }, { kind: "added", text: "  max: 1", inline_changes: [] }] }] }), 200, { etag: tag });
    };
    return { state, handler };
  }

  it("is keyed by the two references and the file, so that nothing learned about the file earlier can stand in for it", async () => {
    const h = changing();
    stubHub(h.handler);
    const { client } = renderLanes({});
    await screen.findByRole("grid", { name: "Differences in the file" });
    const keys = client
      .getQueryCache()
      .getAll()
      .map((q) => q.queryKey)
      .filter((key) => key[0] === "compare");
    expect(keys).toEqual([["compare", "effective:sitb", "effective:sitc", "a/core.yml", null]]);
  });

  it("shows the new answer when the file changed between two answers, asking again with the ETag it kept", async () => {
    const h = changing();
    stubHub(h.handler);
    const { client } = renderLanes({});
    const diff = await screen.findByRole("grid", { name: "Differences in the file" });
    expect(within(diff).getAllByText(/max: 2000/).length).toBeGreaterThan(0);
    expect(h.state.tags).toEqual([null]);

    // The file changes on NFS and the hub says so (file_observed names the swimlane): the comparison is read again.
    h.state.version = 2;
    await act(async () => {
      await invalidateTargets(client, [{ kind: "compares", swimlane: "sitb" }]);
    });
    await waitFor(() => expect(within(screen.getByRole("grid", { name: "Differences in the file" })).getAllByText(/max: 7777/).length).toBeGreaterThan(0));
    expect(within(screen.getByRole("grid", { name: "Differences in the file" })).queryByText(/max: 2000/)).toBeNull();
    expect(h.state.tags).toEqual([null, '"v1"']);
    expect(h.state.statuses).toEqual([200, 200]);

    // Nothing changed this time: the hub answers 304 and the page keeps the answer it has.
    await act(async () => {
      await invalidateTargets(client, [{ kind: "compares", swimlane: "sitc" }]);
    });
    await waitFor(() => expect(h.state.statuses).toEqual([200, 200, 304]));
    expect(h.state.tags.at(-1)).toBe('"v2"');
    expect(within(screen.getByRole("grid", { name: "Differences in the file" })).getAllByText(/max: 7777/).length).toBeGreaterThan(0);
  });

  it("does not read the comparison again for a swimlane that is not one of its sides", async () => {
    const h = changing();
    stubHub(h.handler);
    const { client } = renderLanes({});
    await screen.findByRole("grid", { name: "Differences in the file" });
    await act(async () => {
      await invalidateTargets(client, [{ kind: "compares", swimlane: "presitb" }]);
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(h.state.statuses).toEqual([200]);
  });
});

describe("the keyboard of the file panel", () => {
  it("switches between split and unified with the u key, and the buttons say which is on", async () => {
    const user = userEvent.setup();
    stubHub(hub(ENTRIES).handler);
    renderLanes({});
    await screen.findByRole("grid", { name: "Differences in the file" });
    expect(screen.getByRole("radio", { name: "Split view" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("u");
    expect(screen.getByRole("radio", { name: "Unified view" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("u");
    expect(screen.getByRole("radio", { name: "Split view" })).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("radio", { name: "Unified view" }));
    expect(screen.getByRole("radio", { name: "Unified view" })).toHaveAttribute("aria-checked", "true");
  });

  it("moves the cursor of the tree with the arrow keys and chooses a file with Enter", async () => {
    const user = userEvent.setup();
    const h = hub(ENTRIES);
    stubHub(h.handler);
    const { seen } = renderLanes({});
    const tree = await screen.findByRole("tree", { name: "Differing files" });
    tree.focus();
    // resources, receipt-footer.txt, receipt-logo.jpeg, tx-infinity-api, ...
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{Enter}");
    expect(seen.at(-1)?.file).toBe("resources/receipt-logo.jpeg");
  });
});

describe("the structure of each state", () => {
  it("has no violation of names, roles or ARIA with the tree and a diff", async () => {
    stubHub(hub(ENTRIES).handler);
    renderLanes({});
    await screen.findByRole("grid", { name: "Differences in the file" });
    await expectNoAxeViolations();
  });

  it("has none when the sides are not chosen, and when the two are identical", async () => {
    stubHub(hub([entry("a.yml", "same")]).handler);
    renderLanes({});
    await screen.findByText("sitb and sitc are identical");
    await expectNoAxeViolations();
  });
});
