import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
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
