import { act, fireEvent, render, screen } from "@testing-library/react";
import { createRef, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VirtualList, type VirtualListHandle } from "./VirtualList";

// The virtualiser is the reason a 5,000-row grid and a 6,000-line diff stay responsive: only the rows in view exist in the
// DOM. jsdom has no layout, so src/test/setup.ts gives every element a box (1024x768, or data-test-width and data-test-height)
// and these tests move the scroll position by hand.

const row = (i: number): ReactNode => <span>row {i}</span>;

function scrollTo(el: HTMLElement, top: number): void {
  Object.defineProperty(el, "scrollTop", { configurable: true, writable: true, value: top });
  act(() => {
    fireEvent.scroll(el);
  });
}

const rowNumbers = (): number[] => screen.queryAllByText(/^row \d+$/).map((el) => Number(el.textContent?.replace("row ", "")));

describe("VirtualList", () => {
  it("renders the rows in view plus the overscan, not every row", () => {
    render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={row} data-test-height="320" />);
    const shown = rowNumbers();
    // 320 px is 10 rows; the overscan of 10 adds rows after the last one in view.
    expect(shown[0]).toBe(0);
    expect(shown.length).toBeGreaterThanOrEqual(10);
    expect(shown.length).toBeLessThanOrEqual(25);
  });

  it("renders every row of a short list (a hundred or fewer)", () => {
    render(<VirtualList label="Rows" count={100} rowHeight={32} renderRow={row} data-test-height="160" />);
    expect(rowNumbers()).toHaveLength(100);
  });

  it("starts to virtualise above a hundred rows", () => {
    render(<VirtualList label="Rows" count={101} rowHeight={32} renderRow={row} data-test-height="160" />);
    expect(rowNumbers().length).toBeLessThan(101);
  });

  it("calls renderRow only for rows it shows", () => {
    const renderRow = vi.fn(row);
    render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={renderRow} data-test-height="320" />);
    expect(renderRow.mock.calls.length).toBeLessThan(60);
  });

  it("moves the window when the list scrolls", () => {
    render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={row} data-test-height="320" />);
    scrollTo(screen.getByRole("list", { name: "Rows" }), 32 * 2_000);
    const shown = rowNumbers();
    expect(shown[0]).toBeGreaterThanOrEqual(1_985);
    expect(shown).toContain(2_000);
    expect(shown).not.toContain(0);
  });

  it("sets the size of the whole list, so the scroll bar is the size of 5,000 rows", () => {
    const { container } = render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={row} data-test-height="320" />);
    const spacer = container.querySelector<HTMLElement>("[data-slot='virtual-spacer']");
    expect(spacer?.style.height).toBe(`${5_000 * 32}px`);
  });

  it("gives the container the row count and every row its place in the whole list", () => {
    render(<VirtualList label="Rows" role="grid" rowRole="row" count={5_000} rowHeight={32} renderRow={row} data-test-height="320" />);
    const grid = screen.getByRole("grid", { name: "Rows" });
    expect(grid).toHaveAttribute("aria-rowcount", "5000");
    const rows = screen.getAllByRole("row");
    expect(rows[0]).toHaveAttribute("aria-rowindex", "1");
    expect(rows[3]).toHaveAttribute("aria-rowindex", "4");
  });

  it("counts a header row in the total and starts the row indexes after it", () => {
    render(
      <VirtualList label="Rows" role="grid" rowRole="row" count={50} rowHeight={28} headerHeight={32} header={<div role="row" aria-rowindex={1}>head</div>} renderRow={row} data-test-height="320" />,
    );
    expect(screen.getByRole("grid", { name: "Rows" })).toHaveAttribute("aria-rowcount", "51");
    expect(screen.getAllByRole("row")[1]).toHaveAttribute("aria-rowindex", "2");
  });

  it("keeps the pinned row in the DOM after it scrolls out of view (focus and selection must not vanish)", () => {
    render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={row} pinnedIndex={3} data-test-height="320" />);
    scrollTo(screen.getByRole("list", { name: "Rows" }), 32 * 3_000);
    const shown = rowNumbers();
    expect(shown).toContain(3);
    expect(shown).toContain(3_000);
  });

  it("lets a row take props: an id, a state, a handler", () => {
    const onClick = vi.fn<(index: number) => void>();
    render(
      <VirtualList
        label="Rows"
        count={3}
        rowHeight={32}
        renderRow={row}
        getRowProps={(i) => ({ id: `r-${i}`, "aria-selected": i === 1, onClick: () => onClick(i) })}
        role="listbox"
        rowRole="option"
      />,
    );
    const options = screen.getAllByRole("option");
    expect(options[1]).toHaveAttribute("id", "r-1");
    expect(options[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.click(options[2] as HTMLElement);
    expect(onClick).toHaveBeenCalledWith(2);
  });

  it("supports a height for each row, as the diff has one per kind", () => {
    const heights = [18, 20, 24];
    const { container } = render(<VirtualList label="Rows" count={3} rowHeight={(i) => heights[i] ?? 18} renderRow={row} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("62px");
    const tops = [...container.querySelectorAll<HTMLElement>("[data-slot='virtual-row']")].map((el) => el.style.transform);
    expect(tops).toEqual(["translateY(0px)", "translateY(18px)", "translateY(38px)"]);
  });

  it("follows a change of the count", () => {
    const { container, rerender } = render(<VirtualList label="Rows" count={10} rowHeight={32} renderRow={row} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("320px");
    rerender(<VirtualList label="Rows" count={4} rowHeight={32} renderRow={row} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("128px");
    expect(rowNumbers()).toEqual([0, 1, 2, 3]);
  });

  it("follows a change of the row heights even when the count stays", () => {
    const { container, rerender } = render(<VirtualList label="Rows" count={3} rowHeight={() => 18} renderRow={row} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("54px");
    rerender(<VirtualList label="Rows" count={3} rowHeight={() => 24} renderRow={row} />);
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe("72px");
  });

  it("scrolls to an index on request", () => {
    const ref = createRef<VirtualListHandle>();
    render(<VirtualList ref={ref} label="Rows" count={5_000} rowHeight={32} renderRow={row} data-test-height="320" />);
    const el = screen.getByRole("list", { name: "Rows" });
    const spy = vi.fn();
    Object.defineProperty(el, "scrollTo", { configurable: true, value: spy });
    // The virtualiser clamps to the real scroll extent, which jsdom does not compute.
    Object.defineProperty(el, "scrollHeight", { configurable: true, value: 5_000 * 32 });
    act(() => ref.current?.scrollToIndex(1_000, { align: "start" }));
    expect(spy).toHaveBeenCalled();
    const call = spy.mock.calls.at(-1)?.[0] as { top: number };
    expect(call.top).toBe(32_000);
    expect(ref.current?.element()).toBe(el);
  });

  it("can be focused by keyboard, since it scrolls (axe: scrollable-region-focusable)", () => {
    render(<VirtualList label="Rows" count={5} rowHeight={32} renderRow={row} />);
    expect(screen.getByRole("list", { name: "Rows" })).toHaveAttribute("tabindex", "0");
  });

  it("renders nothing but the container for an empty list", () => {
    const { container } = render(<VirtualList label="Rows" count={0} rowHeight={32} renderRow={row} />);
    expect(container.querySelectorAll("[data-slot='virtual-row']")).toHaveLength(0);
    expect(screen.getByRole("list", { name: "Rows" })).toBeInTheDocument();
  });
});

// Fast scrolling (P10: a 5,000-row grid at 60 fps). A window that moves by many rows in one frame cannot be read, and the cost
// of a frame is the rows that entered it. While the scroll is that fast, every row is drawn as a shell (the content of the rows
// that were on screen goes at once, as they leave the window in the next frames anyway); when the scroll slows or stops, every
// row gets its content. Opt-in, because a list that is not made of costly rows has no need of it.
describe("VirtualList, fast scrolling", () => {
  afterEach(() => vi.useRealTimers());

  const ghostCalls = (renderRow: ReturnType<typeof vi.fn>): Array<[number, boolean]> => renderRow.mock.calls.map((c) => [c[0] as number, c[1] as boolean]);
  const setup = (extra: { ghostWhenFast?: boolean } = {}) => {
    vi.useFakeTimers();
    const renderRow = vi.fn((i: number, ghost: boolean) => <span>{ghost ? "ghost" : `row ${i}`}</span>);
    render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={renderRow} data-test-height="320" ghostWhenFast {...extra} />);
    return { renderRow, list: screen.getByRole("list", { name: "Rows" }) };
  };

  it("draws every row as a shell while the scroll is fast, and calls them ghosts", () => {
    const { renderRow, list } = setup();
    renderRow.mockClear();
    scrollTo(list, 32 * 2_000);
    const calls = ghostCalls(renderRow);
    expect(calls.length).toBeGreaterThan(10);
    expect(calls.every(([, ghost]) => ghost)).toBe(true);
    expect(screen.queryAllByText("ghost").length).toBeGreaterThan(10);
    expect(screen.queryAllByText(/^row \d+$/)).toHaveLength(0);
  });

  it("hides a shell from assistive technology: there is nothing in it to read", () => {
    const { list } = setup();
    scrollTo(list, 32 * 2_000);
    const rows = list.querySelectorAll("[data-slot='virtual-row']");
    expect(rows.length).toBeGreaterThan(10);
    for (const row of rows) expect(row).toHaveAttribute("aria-hidden", "true");
  });

  it("gives every row its content when the scroll has slowed down for a moment", () => {
    const { list } = setup();
    scrollTo(list, 32 * 2_000);
    expect(screen.queryAllByText("ghost").length).toBeGreaterThan(10);
    act(() => {
      vi.advanceTimersByTime(400);
    });
    expect(screen.queryAllByText("ghost")).toHaveLength(0);
    expect(screen.queryAllByText(/^row \d+$/).length).toBeGreaterThan(10);
    for (const row of list.querySelectorAll("[data-slot='virtual-row']")) expect(row).not.toHaveAttribute("aria-hidden");
  });

  it("keeps drawing shells while the scroll stays fast, and waits for it to settle", () => {
    const { list } = setup();
    scrollTo(list, 32 * 1_000);
    act(() => {
      vi.advanceTimersByTime(60);
    });
    scrollTo(list, 32 * 2_000);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    // Not settled yet: the second jump came 60 ms after the first.
    expect(screen.queryAllByText("ghost").length).toBeGreaterThan(10);
    act(() => {
      vi.advanceTimersByTime(400);
    });
    expect(screen.queryAllByText("ghost")).toHaveLength(0);
  });

  it("never draws a shell for a slow scroll: a few rows a frame is read as it goes", () => {
    const { renderRow, list } = setup();
    renderRow.mockClear();
    for (let top = 0; top <= 32 * 40; top += 32 * 2) scrollTo(list, top);
    expect(ghostCalls(renderRow).every(([, ghost]) => !ghost)).toBe(true);
    expect(screen.queryAllByText("ghost")).toHaveLength(0);
  });

  it("takes down the content of the rows that were on screen as well, at once, rather than frame by frame as they leave", () => {
    const { list } = setup();
    // Rows 0 to 19 are drawn with content at rest (ten in view, ten of overscan). Twelve rows down is fast: rows 2 to 19 are
    // still in the window, and they too are shells now.
    expect(screen.getAllByText(/^row \d+$/)).toHaveLength(20);
    scrollTo(list, 32 * 12);
    expect(screen.queryAllByText(/^row \d+$/)).toHaveLength(0);
    expect(screen.getAllByText("ghost")).toHaveLength(30);
  });

  it("is off unless asked for", () => {
    vi.useFakeTimers();
    const renderRow = vi.fn((i: number, _ghost: boolean) => <span>{`row ${i}`}</span>);
    render(<VirtualList label="Rows" count={5_000} rowHeight={32} renderRow={renderRow} data-test-height="320" />);
    scrollTo(screen.getByRole("list", { name: "Rows" }), 32 * 2_000);
    expect(screen.queryAllByText(/^row \d+$/).length).toBeGreaterThan(10);
    expect(renderRow.mock.calls.every((c) => !c[1])).toBe(true);
  });

  it("does not draw shells for a short list that is not virtualised", () => {
    vi.useFakeTimers();
    const renderRow = vi.fn((i: number, ghost: boolean) => <span>{ghost ? "ghost" : `row ${i}`}</span>);
    render(<VirtualList label="Rows" count={100} rowHeight={32} renderRow={renderRow} data-test-height="320" ghostWhenFast />);
    scrollTo(screen.getByRole("list", { name: "Rows" }), 32 * 70);
    expect(screen.queryAllByText("ghost")).toHaveLength(0);
  });
});
