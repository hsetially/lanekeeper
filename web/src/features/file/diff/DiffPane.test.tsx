import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { Comparison, FileContent, FileEntry, SwimlaneDetail } from "@/api/generated/model";
import { HASH_A, HASH_B, problem, renderFile, routes } from "../test-utils";
import { DiffPane, type DiffPaneProps } from "./DiffPane";
import type { Side } from "./refs";

// The Diff tab (File, 1a to 1c, 1m): the comparison the engine computed between this swimlane's NFS and another version, laid
// out as a split or unified diff, with the settings that changed and the notice for a change of line endings only.

const COMPARE = "GET /api/v1/compare";
const LANE = "sitb";
const entry = { path: "tx-infinity-api/tx-infinity-core-sit7.yml", logical_file: "tx-infinity-api/tx-infinity-core.yml", tenant: "sit7", drift: "nfs_ahead" } as unknown as FileEntry;
const detail = { id: LANE, tenants: ["sit7"], agent: "connected", base_version: { tag: "v2.3.1", distance: 4, commit: "a".repeat(40) } } as unknown as SwimlaneDetail;
const TEXT = ["a: 1", "b: 2", "c: 3", "txInfinityOptions:", "  enableAccountSorting: true", "  sortOrder: asc", "g: 7", "h: 8"].join("\r\n");
const head = { path: entry.path, hash: HASH_A, class: "structured", format: { eol: "crlf", encoding: "utf8", bom: false }, size: 100, content_withheld: false, text: TEXT } as FileContent;

const comparison = (over: Partial<Comparison> = {}): Comparison => ({
  left: "git:tenant@sit7",
  right: `nfs:${LANE}`,
  file: "tx-infinity-api/tx-infinity-core.yml",
  class: "structured",
  identical: false,
  eol_only: false,
  hunks: [
    {
      old_start: 4,
      old_lines: 2,
      new_start: 4,
      new_lines: 3,
      lines: [
        { kind: "context", text: "txInfinityOptions:", inline_changes: [] },
        { kind: "removed", text: "  enableAccountSorting: false", inline_changes: [{ start: 24, end: 29 }] },
        { kind: "added", text: "  enableAccountSorting: true", inline_changes: [{ start: 24, end: 28 }] },
        { kind: "added", text: "  sortOrder: asc", inline_changes: [] },
      ],
    },
  ],
  setting_changes: [
    { path: "txInfinityOptions.enableAccountSorting", kind: "changed", left: { text: "false", ty: "bool", flag: null }, right: { text: "true", ty: "bool", flag: null }, left_location: null, right_location: { path: "x", lines: { start: 5, end: 5 } }, severity: null },
    { path: "txInfinityOptions.sortOrder", kind: "added", left: null, right: { text: "asc", ty: "string", flag: null }, left_location: null, right_location: { path: "x", lines: { start: 6, end: 6 } }, severity: null },
  ],
  binary: null,
  ...over,
});

const allowed = { allowed: true, reason: null };

function Harness({ side: initial = { kind: "git" }, ...rest }: Partial<DiffPaneProps> & { side?: Side }) {
  const [side, setSide] = useState<Side>(initial);
  const [swapped, setSwapped] = useState(false);
  const [layout, setLayout] = useState<"split" | "unified">("split");
  return (
    <DiffPane
      lane={LANE}
      path={entry.path}
      name="tx-infinity-core-sit7.yml"
      entry={entry}
      detail={detail}
      head={head}
      side={side}
      swapped={swapped}
      layout={layout}
      onSide={setSide}
      onSwap={() => setSwapped((v) => !v)}
      onLayout={setLayout}
      convertGate={allowed}
      onConvert={() => undefined}
      {...rest}
    />
  );
}

const api = (body: Comparison = comparison()) => routes({ [COMPARE]: () => body, "GET /api/v1/swimlanes": () => ({ items: [], next: null }) });

describe("DiffPane", () => {
  it("asks the engine to compare the tenant's Git head with this swimlane's NFS, for the logical file", async () => {
    const { calls } = renderFile(<Harness />, { api: api() });
    await screen.findByRole("grid", { name: /Diff of tx-infinity-core-sit7\.yml/ });
    const query = new URL(calls.find((c) => c.path.startsWith("/api/v1/compare"))?.path ?? "", "http://x").searchParams;
    expect(Object.fromEntries(query)).toEqual({ left: "git:tenant@sit7", right: "nfs:sitb", file: "tx-infinity-api/tx-infinity-core.yml" });
  });

  it("shows the sides, the hunks the engine computed, and the settings that changed", async () => {
    renderFile(<Harness />, { api: api() });
    const grid = await screen.findByRole("grid", { name: /Diff of/ });
    expect(within(grid).getByText("@@ -4,2 +4,3 @@")).toBeInTheDocument();
    expect(within(grid).getByText("sortOrder: asc", { exact: false })).toBeInTheDocument();
    expect(screen.getByText("1 setting changed, 1 added")).toBeInTheDocument();
    expect(screen.getByText("2 changes")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Left/ })).toHaveTextContent("Git (sit7 head)");
    expect(screen.getAllByText("NFS (current)").length).toBeGreaterThan(0);
  });

  it("wraps the toolbar onto a second line when its words do not fit, instead of making the page scroll sideways (the layout run, labels 40% longer, at 1024 wide)", async () => {
    renderFile(<Harness />, { api: api() });
    await screen.findByRole("grid");
    const bar = screen.getByRole("radiogroup", { name: "Diff layout" }).parentElement;
    expect(bar?.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
    expect(bar?.className).toMatch(/(^|\s)min-h-10(\s|$)/);
    expect(bar?.className).not.toMatch(/(^|\s)h-10(\s|$)/);
  });

  it("flips between side by side and unified, and says which is on", async () => {
    renderFile(<Harness />, { api: api() });
    await screen.findByRole("grid");
    expect(screen.getByRole("radio", { name: "Side by side" })).toBeChecked();
    await userEvent.click(screen.getByRole("radio", { name: "Unified" }));
    expect(screen.getByRole("radio", { name: "Unified" })).toBeChecked();
  });

  it("flips the layout with the u key", async () => {
    renderFile(<Harness />, { api: api() });
    await screen.findByRole("grid");
    await userEvent.keyboard("u");
    expect(screen.getByRole("radio", { name: "Unified" })).toBeChecked();
  });

  it("moves between changes with the buttons, and says where it is", async () => {
    renderFile(<Harness />, { api: api() });
    await screen.findByRole("grid");
    expect(screen.getByText("1 change")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Next/ }));
    expect(await screen.findByText("Change 1 of 1")).toBeInTheDocument();
  });

  // The keymap's "Diff: n / p, next and previous change": the buttons show the keys, and the keys did nothing unless the diff itself
  // had focus (found by the keyboard journey in a real browser, where focus is on the tab or the heading after a navigation).
  const twoRuns = (): Comparison => {
    const base = comparison();
    const first = base.hunks[0];
    if (first === undefined) throw new Error("no hunk");
    return {
      ...base,
      hunks: [
        first,
        {
          old_start: 20,
          old_lines: 1,
          new_start: 21,
          new_lines: 1,
          lines: [
            { kind: "removed", text: "  level: WARN", inline_changes: [] },
            { kind: "added", text: "  level: INFO", inline_changes: [] },
          ],
        },
      ],
    };
  };

  it("moves between changes with n and p from anywhere on the tab, and stops at both ends", async () => {
    renderFile(<Harness />, { api: api(twoRuns()) });
    await screen.findByRole("grid");
    expect(screen.queryByText(/Change \d of 2/)).toBeNull();
    await userEvent.keyboard("n");
    expect(await screen.findByText("Change 1 of 2")).toBeInTheDocument();
    await userEvent.keyboard("n");
    expect(await screen.findByText("Change 2 of 2")).toBeInTheDocument();
    await userEvent.keyboard("n");
    expect(screen.getByText("Change 2 of 2")).toBeInTheDocument();
    await userEvent.keyboard("p");
    expect(await screen.findByText("Change 1 of 2")).toBeInTheDocument();
    await userEvent.keyboard("p");
    expect(screen.getByText("Change 1 of 2")).toBeInTheDocument();
  });

  it("moves only once for a key pressed with the diff itself focused", async () => {
    renderFile(<Harness />, { api: api(twoRuns()) });
    const grid = await screen.findByRole("grid");
    grid.focus();
    await userEvent.keyboard("n");
    expect(await screen.findByText("Change 1 of 2")).toBeInTheDocument();
  });

  it("does not move on n or p typed into a field", async () => {
    renderFile(
      <>
        <input aria-label="A field" />
        <Harness />
      </>,
      { api: api(twoRuns()) },
    );
    await screen.findByRole("grid");
    await userEvent.type(screen.getByRole("textbox", { name: "A field" }), "np");
    expect(screen.queryByText(/Change \d of 2/)).toBeNull();
  });

  it("swaps the sides: the comparison is asked the other way round", async () => {
    const { calls } = renderFile(<Harness />, { api: api() });
    await screen.findByRole("grid");
    await userEvent.click(screen.getByRole("button", { name: "Swap sides" }));
    await waitFor(() => expect(calls.filter((c) => c.path.startsWith("/api/v1/compare"))).toHaveLength(2));
    const second = new URL(calls.filter((c) => c.path.startsWith("/api/v1/compare"))[1]?.path ?? "", "http://x").searchParams;
    expect(second.get("left")).toBe("nfs:sitb");
    expect(second.get("right")).toBe("git:tenant@sit7");
  });

  it("says the two sides are the same when they are", async () => {
    renderFile(<Harness />, { api: api(comparison({ identical: true, hunks: [], setting_changes: [] })) });
    expect(await screen.findByText("No differences")).toBeInTheDocument();
    expect(screen.getByText(/Git \(sit7 head\) and NFS \(current\) are the same\./)).toBeInTheDocument();
  });

  it("hides a change of line endings only behind the notice, and shows the diff on request", async () => {
    renderFile(<Harness />, { api: api(comparison({ eol_only: true })) });
    expect(await screen.findByText("Only line endings differ")).toBeInTheDocument();
    expect(screen.queryByRole("grid")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show diff anyway" }));
    expect(await screen.findByRole("grid")).toBeInTheDocument();
  });

  it("converts the NFS file when asked, and says which ending", async () => {
    const onConvert = vi.fn();
    renderFile(<Harness onConvert={onConvert} />, { api: api(comparison({ eol_only: true })) });
    await userEvent.click(await screen.findByRole("button", { name: "Convert NFS file to LF" }));
    expect(onConvert).toHaveBeenCalledWith("lf");
  });

  it("compares a binary file by picture", async () => {
    renderFile(<Harness name="receipt-logo.jpeg" />, { api: api(comparison({ class: "binary", hunks: [], setting_changes: [], binary: { left: HASH_A, right: HASH_B, left_size: 16384, right_size: 18432 } })) });
    expect(await screen.findByText("Different content")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "receipt-logo.jpeg from NFS (current)" })).toHaveAttribute("src", `/api/v1/blobs/${HASH_B}`);
  });

  it("shows a skeleton while the comparison loads", async () => {
    renderFile(<Harness />, { api: (call) => (call.path.startsWith("/api/v1/compare") ? new Promise<Response>(() => undefined) : (api()(call))) });
    const loading = await screen.findByLabelText("Loading comparison");
    expect(loading).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByRole("grid")).not.toBeInTheDocument();
  });

  it("names the problem and the request id when the comparison cannot be made", async () => {
    renderFile(<Harness />, { api: routes({ [COMPARE]: () => problem(403, "forbidden") }) });
    expect(await screen.findByText("Couldn't load comparison")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("unfolds the unchanged lines from the NFS text when the hunks agree with it", async () => {
    const hunks = comparison().hunks;
    renderFile(<Harness head={{ ...head, text: ["a: 1", "b: 2", "c: 3", "txInfinityOptions:", "  enableAccountSorting: true", "  sortOrder: asc", "g: 7"].join("\n") }} />, { api: api(comparison({ hunks })) });
    const grid = await screen.findByRole("grid");
    await userEvent.click(within(grid).getByRole("button", { name: "3 unchanged lines" }));
    expect(within(grid).getAllByText("a: 1").length).toBeGreaterThan(0);
  });

  it("only counts the folds when the hunks do not match the NFS text (the file moved on)", async () => {
    renderFile(<Harness head={{ ...head, text: "something: else\n" }} />, { api: api() });
    const grid = await screen.findByRole("grid");
    expect(within(grid).queryByRole("button", { name: /unchanged/ })).not.toBeInTheDocument();
    expect(within(grid).getByText("3 unchanged lines")).toBeInTheDocument();
  });

  it("waits for the swimlane before it asks for a comparison", async () => {
    const { calls } = renderFile(<Harness detail={undefined} />, { api: api() });
    expect(await screen.findByLabelText("Loading comparison")).toBeInTheDocument();
    expect(calls.filter((c) => c.path.startsWith("/api/v1/compare"))).toHaveLength(0);
  });

  it("says there is no Git version to compare with when a base file has no base version", async () => {
    renderFile(<Harness entry={{ ...entry, tenant: null }} detail={{ ...detail, base_version: null }} />, { api: api() });
    expect(await screen.findByText("No version to compare with")).toBeInTheDocument();
  });

  it("makes a request only for the sides it can name", async () => {
    const { calls } = renderFile(<Harness side={{ kind: "lane", id: "presita" }} />, { api: api() });
    await screen.findByRole("grid");
    const query = new URL(calls.find((c) => c.path.startsWith("/api/v1/compare"))?.path ?? "", "http://x").searchParams;
    expect(query.get("left")).toBe("nfs:presita");
  });
});

