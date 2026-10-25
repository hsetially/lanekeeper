import { act, fireEvent, render } from "@testing-library/react";
import { StrictMode, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { KeyboardProvider, useShortcut } from "./index";

// The hook binds on mount and unbinds on unmount, once even under StrictMode's double effects, reads the latest handler without
// re-binding, and listens on the window in the capture phase, so that an element that stops propagation cannot hide a key.

function Bound({ onHit, enabled = true }: { onHit: (binding: number) => void; enabled?: boolean }) {
  useShortcut("go-to.swimlanes-compare-feature-status-findings", (hit) => onHit(hit.binding), { enabled });
  return null;
}

describe("useShortcut", () => {
  it("fires a registered row from a real key event on the window", () => {
    const onHit = vi.fn();
    render(
      <KeyboardProvider>
        <Bound onHit={onHit} />
      </KeyboardProvider>,
    );
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "i" });
    expect(onHit).toHaveBeenCalledWith(3);
  });

  it("binds once under StrictMode, so one press is one call", () => {
    const onHit = vi.fn();
    render(
      <StrictMode>
        <KeyboardProvider>
          <Bound onHit={onHit} />
        </KeyboardProvider>
      </StrictMode>,
    );
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onHit).toHaveBeenCalledTimes(1);
  });

  it("stops firing when the component unmounts or is disabled", () => {
    const onHit = vi.fn();
    const { rerender, unmount } = render(
      <KeyboardProvider>
        <Bound onHit={onHit} enabled={false} />
      </KeyboardProvider>,
    );
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onHit).not.toHaveBeenCalled();
    rerender(
      <KeyboardProvider>
        <Bound onHit={onHit} enabled />
      </KeyboardProvider>,
    );
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onHit).toHaveBeenCalledTimes(1);
    unmount();
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onHit).toHaveBeenCalledTimes(1);
  });

  it("calls the latest handler without binding again", () => {
    const seen: number[] = [];
    function Counter() {
      const [n, setN] = useState(0);
      useShortcut("global.shortcuts-sheet", () => {
        seen.push(n);
      });
      return (
        <button
          onClick={() => {
            setN((v) => v + 1);
          }}
        >
          add
        </button>
      );
    }
    const { getByText } = render(
      <KeyboardProvider>
        <Counter />
      </KeyboardProvider>,
    );
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    act(() => {
      getByText("add").click();
    });
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    expect(seen).toEqual([0, 1]);
  });

  it("sees a key that an element in the page stops from propagating (the capture phase)", () => {
    const onHit = vi.fn();
    const { getByTestId } = render(
      <KeyboardProvider>
        <Bound onHit={onHit} />
        <div data-testid="trap" />
      </KeyboardProvider>,
    );
    const trap = getByTestId("trap");
    trap.addEventListener("keydown", (event) => {
      event.stopPropagation();
    });
    fireEvent.keyDown(trap, { key: "g" });
    fireEvent.keyDown(trap, { key: "c" });
    expect(onHit).toHaveBeenCalledWith(1);
  });

  it("does nothing, and does not throw, without a provider", () => {
    const onHit = vi.fn();
    render(<Bound onHit={onHit} />);
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onHit).not.toHaveBeenCalled();
  });
});
