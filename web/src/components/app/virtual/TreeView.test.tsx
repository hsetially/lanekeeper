import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { flattenTree, TreeView, type TreeNode } from "./TreeView";

const tree: TreeNode[] = [
  {
    id: "app",
    label: "app",
    children: [
      { id: "app/a.yml", label: "a.yml" },
      { id: "app/b.yml", label: "b.yml" },
      { id: "app/sub", label: "sub", children: [{ id: "app/sub/c.yml", label: "c.yml" }] },
    ],
  },
  { id: "top.yml", label: "top.yml" },
  { id: "lazy", label: "lazy", lazy: true },
];

function Host({ initial = [] as string[], onSelect = vi.fn(), onToggle = vi.fn(), nodes = tree }: { initial?: string[]; onSelect?: (id: string) => void; onToggle?: (id: string, open: boolean) => void; nodes?: TreeNode[] }) {
  const [expanded, setExpanded] = useState(new Set(initial));
  return (
    <TreeView
      label="Files"
      nodes={nodes}
      expanded={expanded}
      onExpandedChange={(next, id) => {
        setExpanded(next);
        onToggle(id, next.has(id));
      }}
      onSelect={onSelect}
    />
  );
}

const active = (): string => document.getElementById(screen.getByRole("tree").getAttribute("aria-activedescendant") ?? "")?.textContent ?? "";

describe("flattenTree", () => {
  it("lists only what is visible, with the level and the place among the siblings", () => {
    const flat = flattenTree(tree, new Set(["app"]));
    expect(flat.map((f) => [f.node.id, f.level, f.posInSet, f.setSize, f.expanded])).toEqual([
      ["app", 1, 1, 3, true],
      ["app/a.yml", 2, 1, 3, false],
      ["app/b.yml", 2, 2, 3, false],
      ["app/sub", 2, 3, 3, false],
      ["top.yml", 1, 2, 3, false],
      ["lazy", 1, 3, 3, false],
    ]);
  });

  it("shows nothing under a collapsed folder, however deep", () => {
    expect(flattenTree(tree, new Set()).map((f) => f.node.id)).toEqual(["app", "top.yml", "lazy"]);
    expect(flattenTree(tree, new Set(["app/sub"])).map((f) => f.node.id)).toEqual(["app", "top.yml", "lazy"]);
  });

  it("says which rows can open: a folder with children, and a folder whose children are not loaded yet", () => {
    const flat = flattenTree(tree, new Set(["app"]));
    expect(flat.map((f) => [f.node.id, f.expandable])).toEqual([
      ["app", true],
      ["app/a.yml", false],
      ["app/b.yml", false],
      ["app/sub", true],
      ["top.yml", false],
      ["lazy", true],
    ]);
  });

  it("has a parent for every row but the roots", () => {
    const flat = flattenTree(tree, new Set(["app", "app/sub"]));
    expect(flat.map((f) => f.parentId)).toEqual([null, "app", "app", "app", "app/sub", null, null]);
  });
});

describe("TreeView", () => {
  it("is a tree of tree items that say their level, place and state", () => {
    render(<Host initial={["app"]} />);
    const items = screen.getAllByRole("treeitem");
    expect(items.map((i) => i.getAttribute("aria-level"))).toEqual(["1", "2", "2", "2", "1", "1"]);
    expect(items[0]).toHaveAttribute("aria-expanded", "true");
    expect(items[1]).not.toHaveAttribute("aria-expanded");
    expect(items[3]).toHaveAttribute("aria-expanded", "false");
    expect(items[1]).toHaveAttribute("aria-posinset", "1");
    expect(items[1]).toHaveAttribute("aria-setsize", "3");
  });

  it("indents sixteen pixels a level", () => {
    render(<Host initial={["app", "app/sub"]} />);
    const rows = screen.getAllByRole("treeitem");
    const pad = (el: HTMLElement) => (el.firstElementChild as HTMLElement).style.paddingLeft;
    expect(pad(rows[0] as HTMLElement)).toBe("8px");
    expect(pad(rows[1] as HTMLElement)).toBe("24px");
    expect(pad(rows[4] as HTMLElement)).toBe("40px");
  });

  it("opens and closes a folder from the keyboard: Right opens, Right again enters, Left closes, Left again goes up", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    render(<Host onToggle={onToggle} />);
    await user.click(screen.getByRole("tree"));
    await user.keyboard("{ArrowDown}");
    expect(active()).toContain("app");
    await user.keyboard("{ArrowRight}");
    expect(onToggle).toHaveBeenLastCalledWith("app", true);
    expect(screen.getByText("a.yml")).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    expect(active()).toContain("a.yml");
    await user.keyboard("{ArrowLeft}");
    expect(active()).toContain("app");
    await user.keyboard("{ArrowLeft}");
    expect(onToggle).toHaveBeenLastCalledWith("app", false);
    expect(screen.queryByText("a.yml")).not.toBeInTheDocument();
  });

  it("moves with Up, Down, Home and End, over the rows that are visible", async () => {
    const user = userEvent.setup();
    render(<Host initial={["app"]} />);
    await user.click(screen.getByRole("tree"));
    await user.keyboard("{End}");
    expect(active()).toContain("lazy");
    await user.keyboard("{Home}");
    expect(active()).toContain("app");
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowUp}");
    expect(active()).toContain("a.yml");
  });

  it("selects with Enter and with a click, and a click on the chevron opens without selecting", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    const onToggle = vi.fn();
    render(<Host onSelect={onSelect} onToggle={onToggle} />);
    await user.click(screen.getByRole("tree"));
    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(onSelect).toHaveBeenLastCalledWith("top.yml");
    await user.click(screen.getByText("lazy"));
    expect(onSelect).toHaveBeenLastCalledWith("lazy");
    onSelect.mockClear();
    await user.click(screen.getByRole("button", { name: "Open app" }));
    expect(onToggle).toHaveBeenLastCalledWith("app", true);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("opens a folder whose children are not loaded yet, and tells the screen so it can fetch them", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    render(<Host onToggle={onToggle} />);
    await user.click(screen.getByRole("button", { name: "Open lazy" }));
    expect(onToggle).toHaveBeenCalledWith("lazy", true);
  });

  it("draws only the rows in view for a large tree, and keeps the active one", async () => {
    const user = userEvent.setup();
    const big: TreeNode[] = [{ id: "root", label: "root", children: Array.from({ length: 5_000 }, (_, i) => ({ id: `f${i}`, label: `file-${i}.yml` })) }];
    render(<Host initial={["root"]} nodes={big} />);
    expect(screen.getAllByRole("treeitem").length).toBeLessThan(80);
    await user.click(screen.getByRole("tree"));
    await user.keyboard("{End}");
    expect(active()).toContain("file-4999.yml");
    const tree = screen.getByRole("tree");
    Object.defineProperty(tree, "scrollTop", { configurable: true, writable: true, value: 0 });
    act(() => {
      fireEvent.scroll(tree);
    });
    expect(document.getElementById(tree.getAttribute("aria-activedescendant") ?? "")).toBeInTheDocument();
  });

  it("shows what a label is as text, never as markup (a file name is data)", () => {
    render(<Host nodes={[{ id: "x", label: "<img src=x onerror=alert(1)>.yml" }]} />);
    expect(screen.getByText("<img src=x onerror=alert(1)>.yml")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("passes the axe structure checks", async () => {
    const { container } = render(<Host initial={["app", "app/sub"]} />);
    await expectNoAxeViolations(container);
  });
});
