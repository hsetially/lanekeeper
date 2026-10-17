import { act, render, renderHook, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearDirtyForTests, markDirty } from "@/auth/dirty";
import { NewVersionBanner, NewVersionScreen } from "@/components/app/NewVersion";
import { installStaleChunkListener, isStale, resetStaleForTests, useStale } from "./stale";

// After a deploy the files of the old version are gone. A tab that was open before it finds out the first time it asks for
// a chunk (`vite:preloadError`) or the first time a route's import fails. Either way the person is told, once, that a new
// version is available, and "Reload" fetches everything fresh, except while they have unsaved work (the dirty registry).

afterEach(() => {
  resetStaleForTests();
  clearDirtyForTests();
});

describe("the stale flag", () => {
  it("is off until a chunk fails to preload, and then on", () => {
    const uninstall = installStaleChunkListener();
    expect(isStale()).toBe(false);
    window.dispatchEvent(new Event("vite:preloadError"));
    expect(isStale()).toBe(true);
    uninstall();
  });

  it("is not set by a listener that was taken off", () => {
    installStaleChunkListener()();
    window.dispatchEvent(new Event("vite:preloadError"));
    expect(isStale()).toBe(false);
  });

  it("listens once however many times it is installed", () => {
    const add = vi.spyOn(window, "addEventListener");
    const a = installStaleChunkListener();
    const b = installStaleChunkListener();
    expect(add.mock.calls.filter(([type]) => type === "vite:preloadError")).toHaveLength(1);
    a();
    b();
    add.mockRestore();
  });

  it("does not stop the error: the import that failed still fails, and the router shows its error screen", () => {
    const uninstall = installStaleChunkListener();
    const event = new Event("vite:preloadError", { cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    uninstall();
  });

  it("tells a component the moment it turns on", () => {
    const uninstall = installStaleChunkListener();
    const { result } = renderHook(() => useStale());
    expect(result.current).toBe(false);
    act(() => {
      window.dispatchEvent(new Event("vite:preloadError"));
    });
    expect(result.current).toBe(true);
    uninstall();
  });
});

describe("NewVersionBanner", () => {
  it("is nothing while the version is current", () => {
    const { container } = render(<NewVersionBanner reload={() => undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("says a new version is available and offers to reload once a chunk has failed", async () => {
    const user = userEvent.setup();
    const reload = vi.fn();
    const uninstall = installStaleChunkListener();
    render(<NewVersionBanner reload={reload} />);
    act(() => {
      window.dispatchEvent(new Event("vite:preloadError"));
    });
    expect(screen.getByText("A new version is available")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Reload" }));
    expect(reload).toHaveBeenCalledTimes(1);
    uninstall();
  });

  it("does not reload while there is unsaved work, and says why", async () => {
    const user = userEvent.setup();
    const reload = vi.fn();
    const uninstall = installStaleChunkListener();
    const release = markDirty("edit-1");
    render(<NewVersionBanner reload={reload} />);
    act(() => {
      window.dispatchEvent(new Event("vite:preloadError"));
    });
    const button = screen.getByRole("button", { name: "Reload" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
    await user.click(button);
    expect(reload).not.toHaveBeenCalled();
    act(() => release());
    expect(screen.getByRole("button", { name: "Reload" })).not.toHaveAttribute("aria-disabled", "true");
    uninstall();
  });
});

describe("NewVersionScreen", () => {
  it("makes the banner step aside while it is on screen, and bring it back when it goes", () => {
    const uninstall = installStaleChunkListener();
    render(<NewVersionBanner reload={() => undefined} />);
    act(() => {
      window.dispatchEvent(new Event("vite:preloadError"));
    });
    expect(screen.getByText("A new version is available")).toBeInTheDocument();
    const page = render(<NewVersionScreen reload={() => undefined} />);
    expect(screen.getAllByRole("button", { name: "Reload" })).toHaveLength(1);
    page.unmount();
    expect(screen.getAllByRole("button", { name: "Reload" })).toHaveLength(1);
    expect(screen.getByRole("status")).toBeInTheDocument();
    uninstall();
  });

  it("is a page of its own with a heading, for a route whose chunk could not be fetched", async () => {
    const user = userEvent.setup();
    const reload = vi.fn();
    render(<NewVersionScreen reload={reload} />);
    expect(screen.getByRole("heading", { level: 1, name: "A new version is available" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Reload" }));
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("holds the reload back while there is unsaved work", async () => {
    const user = userEvent.setup();
    const reload = vi.fn();
    markDirty("edit-1");
    render(<NewVersionScreen reload={reload} />);
    await user.click(screen.getByRole("button", { name: "Reload" }));
    expect(reload).not.toHaveBeenCalled();
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
  });
});
