import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ToastStore } from "./store";
import { ToasterView } from "./Toaster";

// The toaster is a persistent polite region: a new toast is announced, hovering or focusing it keeps it on screen, it goes
// by itself after its time (an error never does), and it has a Dismiss button and an optional link or button.

let store: ToastStore;
beforeEach(() => {
  vi.useFakeTimers();
  store = new ToastStore();
});
afterEach(() => {
  vi.useRealTimers();
});

const renderToaster = () => render(<ToasterView store={store} />);

describe("ToasterView", () => {
  it("is a named region with a persistent polite live area, so an inserted toast is announced", () => {
    renderToaster();
    const region = screen.getByRole("region", { name: "Notifications" });
    expect(region.querySelector("[aria-live='polite']")).not.toBeNull();
  });

  it("shows a toast with its title, a link and Dismiss", () => {
    renderToaster();
    act(() => {
      store.push({ title: "sitb: file.yml changed on NFS", action: { label: "View swimlane", href: "/swimlanes/sitb" } });
    });
    const region = screen.getByRole("region", { name: "Notifications" });
    expect(within(region).getByText("sitb: file.yml changed on NFS")).toBeInTheDocument();
    expect(within(region).getByRole("link", { name: "View swimlane" })).toHaveAttribute("href", "/swimlanes/sitb");
    expect(within(region).getByRole("button", { name: "Dismiss" })).toBeInTheDocument();
  });

  it("runs a button action and then dismisses the toast", () => {
    renderToaster();
    const onClick = vi.fn();
    act(() => {
      store.push({ title: "t", action: { label: "Undo", onClick } });
    });
    act(() => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(store.getSnapshot()).toEqual([]);
  });

  it("dismisses on the Dismiss button", async () => {
    vi.useRealTimers();
    renderToaster();
    act(() => {
      store.push({ title: "t" });
    });
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(store.getSnapshot()).toEqual([]);
  });

  it("goes by itself after its time, and an error does not", () => {
    renderToaster();
    act(() => {
      store.push({ title: "quick", durationMs: 1000 });
      store.push({ title: "failure", kind: "danger" });
    });
    act(() => {
      vi.advanceTimersByTime(1001);
    });
    expect(screen.queryByText("quick")).toBeNull();
    expect(screen.getByText("failure")).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(10 * 60 * 1000);
    });
    expect(screen.getByText("failure")).toBeInTheDocument();
  });

  it("stays while the pointer is over it or focus is inside it, and then runs its time again", () => {
    renderToaster();
    act(() => {
      store.push({ title: "held", durationMs: 1000 });
    });
    const toast = screen.getByText("held").closest("[data-slot='toast']") as HTMLElement;
    act(() => {
      toast.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(screen.getByText("held")).toBeInTheDocument();
    act(() => {
      toast.dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(1001);
    });
    expect(screen.queryByText("held")).toBeNull();
  });

  it("marks the kind by an icon and by words for assistive technology, not by colour alone", () => {
    renderToaster();
    act(() => {
      store.push({ title: "Saved", kind: "success" });
      store.push({ title: "Broke", kind: "danger" });
    });
    const toasts = [...document.querySelectorAll("[data-slot='toast']")];
    expect(toasts).toHaveLength(2);
    for (const t of toasts) {
      expect(t.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
      expect(t.querySelector(".sr-only")).not.toBeNull();
    }
    expect(toasts[0]?.querySelector(".sr-only")).toHaveTextContent("Success");
    expect(toasts[1]?.querySelector(".sr-only")).toHaveTextContent("Error");
  });
});

describe("ToasterView links", () => {
  it("hands a plain click on a link to the app's router, and leaves a modified click to the browser", () => {
    const onNavigate = vi.fn();
    render(<ToasterView store={store} onNavigate={onNavigate} />);
    act(() => {
      store.push({ title: "t", action: { label: "View swimlane", href: "/swimlanes/sitb" } });
    });
    const link = screen.getByRole("link", { name: "View swimlane" });
    act(() => {
      link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true, button: 0 }));
    });
    expect(onNavigate).not.toHaveBeenCalled();
    // Following the link, in this tab or another, is the end of the notice.
    expect(store.getSnapshot()).toEqual([]);
    act(() => {
      store.push({ title: "again", action: { label: "View swimlane", href: "/swimlanes/sitb" } });
    });
    act(() => {
      screen.getByRole("link", { name: "View swimlane" }).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
    });
    expect(onNavigate).toHaveBeenCalledWith("/swimlanes/sitb");
    expect(store.getSnapshot()).toEqual([]);
  });
});
