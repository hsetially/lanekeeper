import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import type { PanelImperativeHandle } from "react-resizable-panels";
import { describe, expect, it, vi } from "vitest";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "./resizable";

describe("Resizable", () => {
  it("renders two panels split by a focusable separator with a name and range", () => {
    render(
      <ResizablePanelGroup orientation="horizontal" id="files">
        <ResizablePanel id="list" defaultSize="30%" minSize="15%">
          list
        </ResizablePanel>
        <ResizableHandle aria-label="Resize file list" id="handle" />
        <ResizablePanel id="detail">detail</ResizablePanel>
      </ResizablePanelGroup>,
    );
    expect(screen.getByText("list")).toBeInTheDocument();
    expect(screen.getByText("detail")).toBeInTheDocument();
    const handle = screen.getByRole("separator", { name: "Resize file list" });
    expect(handle).toHaveAttribute("tabindex", "0");
    expect(handle).toHaveAttribute("aria-orientation", "vertical");
    expect(handle).toHaveAttribute("aria-valuenow");
  });
});

// The keymap's "Split pane handle: Left and Right resize 16 px, with Shift 64 px". The library steps by a percentage of the
// group, so a handle that is given the panel before it steps by pixels itself. Found by the keyboard journey in a real browser,
// where one press of the arrow moved the file tree of a swimlane by 44 px.
describe("a handle with a panel to step", () => {
  function setup(width = 400, collapsed = false) {
    const panel = createRef<PanelImperativeHandle>();
    const resize = vi.fn();
    const expand = vi.fn();
    panel.current = { collapse: vi.fn(), expand, isCollapsed: () => collapsed, getSize: () => ({ inPixels: collapsed ? 0 : width, asPercentage: 40 }), resize };
    render(
      <ResizablePanelGroup orientation="horizontal" id="files">
        <ResizablePanel id="list" defaultSize="40%">
          list
        </ResizablePanel>
        <ResizableHandle aria-label="Resize file list" id="handle" panel={panel} />
        <ResizablePanel id="detail">detail</ResizablePanel>
      </ResizablePanelGroup>,
    );
    screen.getByRole("separator", { name: "Resize file list" }).focus();
    return Object.assign(resize, { expand });
  }

  it("moves the panel by 16 pixels on Left and Right", async () => {
    const resize = setup(400);
    await userEvent.keyboard("{ArrowRight}");
    expect(resize).toHaveBeenLastCalledWith(416);
    await userEvent.keyboard("{ArrowLeft}");
    expect(resize).toHaveBeenLastCalledWith(384);
  });

  it("moves it by 64 pixels with Shift", async () => {
    const resize = setup(400);
    await userEvent.keyboard("{Shift>}{ArrowRight}{/Shift}");
    expect(resize).toHaveBeenLastCalledWith(464);
    await userEvent.keyboard("{Shift>}{ArrowLeft}{/Shift}");
    expect(resize).toHaveBeenLastCalledWith(336);
  });

  it("leaves the other keys, and a chord, to the library", async () => {
    const resize = setup(400);
    await userEvent.keyboard("{Home}{End}{Control>}{ArrowRight}{/Control}{Alt>}{ArrowLeft}{/Alt}{ArrowUp}{ArrowDown}");
    expect(resize).not.toHaveBeenCalled();
  });

  // The keymap: "Home/End collapse". A collapsible panel collapses on Home (the library's own key), and then has no width to step
  // from: Right brings it back, as the panel was before (a step of 16 px from nothing would land inside the range where the
  // library snaps it shut again).
  it("brings a collapsed panel back on Right, and does nothing on Left (there is nothing to take away)", async () => {
    const resize = setup(400, true);
    await userEvent.keyboard("{ArrowLeft}");
    expect(resize).not.toHaveBeenCalled();
    expect(resize.expand).not.toHaveBeenCalled();
    await userEvent.keyboard("{ArrowRight}");
    expect(resize.expand).toHaveBeenCalledTimes(1);
    expect(resize).not.toHaveBeenCalled();
  });

  it("brings it back on Shift and Right too", async () => {
    const resize = setup(400, true);
    await userEvent.keyboard("{Shift>}{ArrowRight}{/Shift}");
    expect(resize.expand).toHaveBeenCalledTimes(1);
  });
});
