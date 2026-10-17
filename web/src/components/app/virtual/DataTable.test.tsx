import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { DataTable, type Column, type SortState } from "./DataTable";

type Item = { path: string; lines: number };
const items = (n: number): Item[] => Array.from({ length: n }, (_, i) => ({ path: `dir/file-${i}.yml`, lines: i * 3 }));
const columns: Column<Item>[] = [
  { id: "path", header: "Path", width: "minmax(0,1fr)", sortable: true, cell: (r) => r.path },
  { id: "lines", header: "Lines", width: "80px", align: "right", cell: (r) => String(r.lines) },
];
const key = (r: Item): string => r.path;

function scrollTo(el: HTMLElement, top: number): void {
  Object.defineProperty(el, "scrollTop", { configurable: true, writable: true, value: top });
  act(() => {
    fireEvent.scroll(el);
  });
}

describe("DataTable", () => {
  it("is a grid with a header row, rows and cells, and says how many rows there are in all", () => {
    render(<DataTable label="Files" rows={items(5_000)} columns={columns} rowKey={key} data-test-height="320" />);
    const grid = screen.getByRole("grid", { name: "Files" });
    expect(grid).toHaveAttribute("aria-rowcount", "5001");
    expect(screen.getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Path", "Lines"]);
    const rows = screen.getAllByRole("row");
    expect(rows[0]).toHaveAttribute("aria-rowindex", "1");
    expect(rows[1]).toHaveAttribute("aria-rowindex", "2");
    expect(within(rows[1] as HTMLElement).getAllByRole("gridcell").map((c) => c.textContent)).toEqual(["dir/file-0.yml", "0"]);
    expect(rows.length).toBeLessThan(40);
  });

  it("uses 32 px rows by default and 28 px rows when compact (the handoff's two densities)", () => {
    const { container, rerender } = render(<DataTable label="Files" rows={items(3)} columns={columns} rowKey={key} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("96px");
    rerender(<DataTable label="Files" rows={items(3)} columns={columns} rowKey={key} density="compact" />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("84px");
  });

  it("puts the columns' widths in one variable on the container, so a row sets none of its own", () => {
    const { container } = render(<DataTable label="Files" rows={items(3)} columns={columns} rowKey={key} />);
    const list = container.querySelector<HTMLElement>("[data-slot='virtual-list']");
    expect(list?.style.getPropertyValue("--lk-cols")).toBe("minmax(0,1fr) 80px");
  });

  it("scrolls sideways when the columns do not fit (forty swimlanes)", () => {
    const wide: Column<Item>[] = Array.from({ length: 41 }, (_, i) => ({ id: `c${i}`, header: `c${i}`, width: "120px", minWidth: 120, cell: () => "x" }));
    const { container } = render(<DataTable label="Grid" rows={items(3)} columns={wide} rowKey={key} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-header']")?.style.minWidth).toBe("4920px");
  });

  it("sorts: a click on a sortable header asks for ascending, then descending, then none", async () => {
    const user = userEvent.setup();
    const onSortChange = vi.fn<(next: SortState | null) => void>();
    function Host() {
      const [sort, setSort] = useState<SortState | null>(null);
      return (
        <DataTable
          label="Files"
          rows={items(3)}
          columns={columns}
          rowKey={key}
          sort={sort}
          onSortChange={(next) => {
            onSortChange(next);
            setSort(next);
          }}
        />
      );
    }
    render(<Host />);
    const header = screen.getByRole("columnheader", { name: /Path/ });
    expect(header).toHaveAttribute("aria-sort", "none");
    await user.click(within(header).getByRole("button"));
    expect(header).toHaveAttribute("aria-sort", "ascending");
    await user.click(within(header).getByRole("button"));
    expect(header).toHaveAttribute("aria-sort", "descending");
    await user.click(within(header).getByRole("button"));
    expect(header).toHaveAttribute("aria-sort", "none");
    expect(onSortChange.mock.calls.map((c) => c[0])).toEqual([{ column: "path", direction: "asc" }, { column: "path", direction: "desc" }, null]);
    expect(within(screen.getByRole("columnheader", { name: "Lines" })).queryByRole("button")).not.toBeInTheDocument();
  });

  it("moves the active row with the arrow keys, and the grid names it for assistive technology", async () => {
    const user = userEvent.setup();
    render(<DataTable label="Files" rows={items(500)} columns={columns} rowKey={key} data-test-height="320" />);
    const grid = screen.getByRole("grid", { name: "Files" });
    await user.click(grid);
    await user.keyboard("{ArrowDown}");
    const first = grid.getAttribute("aria-activedescendant");
    expect(first).toBeTruthy();
    expect(document.getElementById(first ?? "")).toHaveTextContent("dir/file-0.yml");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(document.getElementById(grid.getAttribute("aria-activedescendant") ?? "")).toHaveTextContent("dir/file-2.yml");
    await user.keyboard("{ArrowUp}");
    expect(document.getElementById(grid.getAttribute("aria-activedescendant") ?? "")).toHaveTextContent("dir/file-1.yml");
  });

  it("jumps with Home, End, Page Down and Page Up, and the row it lands on is rendered", async () => {
    const user = userEvent.setup();
    render(<DataTable label="Files" rows={items(5_000)} columns={columns} rowKey={key} data-test-height="320" />);
    const grid = screen.getByRole("grid", { name: "Files" });
    await user.click(grid);
    await user.keyboard("{End}");
    expect(document.getElementById(grid.getAttribute("aria-activedescendant") ?? "")).toHaveTextContent("dir/file-4999.yml");
    await user.keyboard("{Home}");
    expect(document.getElementById(grid.getAttribute("aria-activedescendant") ?? "")).toHaveTextContent("dir/file-0.yml");
    await user.keyboard("{PageDown}");
    const afterPage = grid.getAttribute("aria-activedescendant") ?? "";
    expect(document.getElementById(afterPage)?.textContent).toMatch(/file-(8|9)\.yml/);
    await user.keyboard("{PageUp}");
    expect(document.getElementById(grid.getAttribute("aria-activedescendant") ?? "")).toHaveTextContent("dir/file-0.yml");
  });

  it("keeps the active row rendered when the list is scrolled away from it", async () => {
    const user = userEvent.setup();
    render(<DataTable label="Files" rows={items(5_000)} columns={columns} rowKey={key} data-test-height="320" />);
    const grid = screen.getByRole("grid", { name: "Files" });
    await user.click(grid);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    const id = grid.getAttribute("aria-activedescendant") ?? "";
    scrollTo(grid, 32 * 3_000);
    expect(document.getElementById(id)).toBeInTheDocument();
  });

  it("activates the active row with Enter and toggles its selection with Space", async () => {
    const user = userEvent.setup();
    const onActivate = vi.fn();
    const onToggleSelect = vi.fn();
    render(<DataTable label="Files" rows={items(10)} columns={columns} rowKey={key} onActivate={onActivate} onToggleSelect={onToggleSelect} />);
    await user.click(screen.getByRole("grid", { name: "Files" }));
    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(onActivate).toHaveBeenCalledWith(expect.objectContaining({ path: "dir/file-1.yml" }), 1);
    await user.keyboard(" ");
    expect(onToggleSelect).toHaveBeenCalledWith(expect.objectContaining({ path: "dir/file-1.yml" }), 1);
  });

  it("marks selected rows with aria-selected", () => {
    render(<DataTable label="Files" rows={items(3)} columns={columns} rowKey={key} selectedKeys={new Set(["dir/file-1.yml"])} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((r) => r.getAttribute("aria-selected"))).toEqual(["false", "true", "false"]);
  });

  it("says how many rows are shown, and loads more on request", async () => {
    const user = userEvent.setup();
    const onLoadMore = vi.fn();
    render(<DataTable label="Files" rows={items(100)} columns={columns} rowKey={key} footer={{ hasMore: true, onLoadMore, loading: false }} />);
    expect(screen.getByText("Showing 100")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Load 100 more" }));
    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("does not offer more when there is none, and says when the list was cut off", () => {
    render(<DataTable label="Files" rows={items(100)} columns={columns} rowKey={key} footer={{ hasMore: false, truncated: true, onLoadMore: () => undefined, loading: false }} />);
    expect(screen.queryByRole("button", { name: /Load/ })).not.toBeInTheDocument();
    expect(screen.getByText("Showing the first 100. The list was cut off.")).toBeInTheDocument();
  });

  it("puts the keyboard on the first new row after more rows are loaded", async () => {
    const user = userEvent.setup();
    function Host() {
      const [n, setN] = useState(100);
      return <DataTable label="Files" rows={items(n)} columns={columns} rowKey={key} footer={{ hasMore: true, loading: false, onLoadMore: () => setN((v) => v + 100) }} />;
    }
    render(<Host />);
    await user.click(screen.getByRole("button", { name: "Load 100 more" }));
    const grid = screen.getByRole("grid", { name: "Files" });
    await waitFor(() => expect(document.activeElement).toBe(grid));
    expect(document.getElementById(grid.getAttribute("aria-activedescendant") ?? "")).toHaveTextContent("dir/file-100.yml");
    expect(screen.getByText("Showing 200")).toBeInTheDocument();
  });

  it("shows a skeleton of the real layout while it loads, marked busy", () => {
    const { container } = render(<DataTable label="Files" rows={[]} columns={columns} rowKey={key} state="loading" />);
    expect(screen.getByRole("grid", { name: "Files" })).toHaveAttribute("aria-busy", "true");
    expect(container.querySelectorAll("[data-slot='skeleton']").length).toBeGreaterThanOrEqual(6);
    expect(screen.getAllByRole("columnheader")).toHaveLength(2);
  });

  it("shows what the caller gives for an empty table and for an error", () => {
    const { rerender } = render(<DataTable label="Files" rows={[]} columns={columns} rowKey={key} empty={<p>No files here</p>} />);
    expect(screen.getByText("No files here")).toBeInTheDocument();
    rerender(<DataTable label="Files" rows={[]} columns={columns} rowKey={key} state="error" error={<p>Could not load</p>} />);
    expect(screen.getByText("Could not load")).toBeInTheDocument();
  });

  it("does not run a cell again for a row that did not change when another row arrives", () => {
    const cell = vi.fn((r: Item) => r.path);
    const cols: Column<Item>[] = [{ id: "path", header: "Path", width: "1fr", cell }];
    const rows = items(10);
    const { rerender } = render(<DataTable label="Files" rows={rows} columns={cols} rowKey={key} />);
    const before = cell.mock.calls.length;
    rerender(<DataTable label="Files" rows={[...rows, { path: "dir/new.yml", lines: 1 }]} columns={cols} rowKey={key} />);
    expect(cell.mock.calls.length - before).toBe(1);
  });

  describe("with many columns (forty swimlanes)", () => {
    const wide = (n: number): Column<Item>[] => Array.from({ length: n }, (_, i) => ({ id: `c${i}`, header: `lane ${i}`, width: i === 0 ? "320px" : "120px", minWidth: i === 0 ? 320 : 120, cell: (r) => (i === 0 ? r.path : `v${i}`) }));
    const lane = (name: string) => screen.queryAllByRole("gridcell").filter((c) => c.textContent === name);

    it("draws only the columns in view, with the first one kept, and says how many there are in all", () => {
      render(<DataTable label="Grid" rows={items(3)} columns={wide(41)} rowKey={key} data-test-width="1000" />);
      const grid = screen.getByRole("grid", { name: "Grid" });
      expect(grid).toHaveAttribute("aria-colcount", "41");
      const firstRow = screen.getAllByRole("row")[1] as HTMLElement;
      const cells = within(firstRow).getAllByRole("gridcell");
      // 1000 px wide: the first column (320), then 680 px of 120 px columns, 6 of them, and two more each side of overscan.
      expect(cells.length).toBeGreaterThanOrEqual(7);
      expect(cells.length).toBeLessThan(15);
      expect(cells[0]).toHaveTextContent("dir/file-0.yml");
      expect(cells[0]).toHaveAttribute("aria-colindex", "1");
      expect(cells[1]).toHaveAttribute("aria-colindex", "2");
    });

    it("names each drawn column by its place among all of them, header and cells alike", () => {
      render(<DataTable label="Grid" rows={items(2)} columns={wide(41)} rowKey={key} data-test-width="1000" />);
      const heads = screen.getAllByRole("columnheader");
      expect(heads[0]).toHaveAttribute("aria-colindex", "1");
      expect(heads.map((h) => h.getAttribute("aria-colindex"))).toEqual(heads.map((_, i) => String(i + 1)));
      expect(heads.length).toBeLessThan(15);
    });

    it("moves the columns in view when the table scrolls sideways, and keeps the first column", () => {
      render(<DataTable label="Grid" rows={items(3)} columns={wide(41)} rowKey={key} data-test-width="1000" />);
      const grid = screen.getByRole("grid", { name: "Grid" });
      expect(lane("v30")).toHaveLength(0);
      Object.defineProperty(grid, "scrollLeft", { configurable: true, writable: true, value: 320 + 120 * 28 });
      act(() => {
        fireEvent.scroll(grid);
      });
      expect(lane("v30").length).toBeGreaterThan(0);
      expect(lane("v2")).toHaveLength(0);
      const firstRow = screen.getAllByRole("row")[1] as HTMLElement;
      expect(within(firstRow).getAllByRole("gridcell")[0]).toHaveTextContent("dir/file-0.yml");
    });

    it("keeps the width of the whole table, so the scroll bar is the size of all the columns", () => {
      const { container } = render(<DataTable label="Grid" rows={items(3)} columns={wide(41)} rowKey={key} data-test-width="1000" />);
      expect(container.querySelector<HTMLElement>("[data-slot='virtual-header']")?.style.minWidth).toBe(`${320 + 40 * 120}px`);
    });

    it("puts the columns in view into one variable, so a row sets no width of its own", () => {
      const { container } = render(<DataTable label="Grid" rows={items(3)} columns={wide(41)} rowKey={key} data-test-width="1000" />);
      const template = container.querySelector<HTMLElement>("[data-slot='virtual-list']")?.style.getPropertyValue("--lk-cols") ?? "";
      const tracks = template.split(" ");
      expect(tracks[0]).toBe("320px");
      // The column of space for the columns scrolled past, and then one 120 px track for each column drawn.
      expect(tracks[1]).toBe("0px");
      expect(tracks.slice(2).every((t) => t === "120px")).toBe(true);
      expect(tracks.length - 2).toBe(within(screen.getAllByRole("row")[1] as HTMLElement).getAllByRole("gridcell").length - 1);
    });

    it("draws every column of a table that is not wide", () => {
      render(<DataTable label="Grid" rows={items(2)} columns={wide(8)} rowKey={key} data-test-width="400" />);
      expect(within(screen.getAllByRole("row")[1] as HTMLElement).getAllByRole("gridcell")).toHaveLength(8);
    });

    it("passes the axe structure checks", async () => {
      const { container } = render(<DataTable label="Grid" rows={items(20)} columns={wide(41)} rowKey={key} data-test-width="1000" data-test-height="320" />);
      await expectNoAxeViolations(container);
    });
  });

  it("passes the axe structure checks, with rows and without", async () => {
    const { container, rerender } = render(<DataTable label="Files" rows={items(30)} columns={columns} rowKey={key} data-test-height="320" footer={{ hasMore: true, loading: false, onLoadMore: () => undefined }} />);
    await expectNoAxeViolations(container);
    rerender(<DataTable label="Files" rows={[]} columns={columns} rowKey={key} empty={<p>No files here</p>} />);
    await expectNoAxeViolations(container);
  });
});
