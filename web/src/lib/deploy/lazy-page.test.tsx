import { act, render, screen, waitFor, type RenderResult } from "@testing-library/react";
import { Component, Suspense, type ComponentType, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { isStale, resetStaleForTests } from "./stale";
import { lazyPage } from "./lazy-page";

// TanStack Router's own `lazyRouteComponent` reloads the page, once, when a chunk is missing (it keeps a flag in
// sessionStorage). That would throw away an unsaved edit and spend the storage the app keeps to `lib/prefs`. This replaces it:
// same `.preload`, no reload. A missing chunk becomes an error the router shows as "A new version is available".

afterEach(() => {
  resetStaleForTests();
  vi.restoreAllMocks();
});

class Boundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  override state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  override render() {
    return this.state.error ? <p>failed: {this.state.error.message}</p> : this.props.children;
  }
}

const chunkError = () => new TypeError("Failed to fetch dynamically imported module: https://x/assets/Files-1.js");
// A component that suspends needs an awaited act, or React says it was not given the chance to settle.
async function mount(node: ReactNode): Promise<RenderResult> {
  let view: RenderResult | undefined;
  await act(async () => {
    view = render(
      <Boundary>
        <Suspense fallback={<p>loading</p>}>{node}</Suspense>
      </Boundary>,
    );
    await Promise.resolve();
  });
  if (!view) throw new Error("not rendered");
  return view;
}
const instant = { sleep: () => Promise.resolve(), random: () => 0 };
/** A loader whose import fails with `error`. */
const failing = (error: Error) => (): Promise<{ default: ComponentType }> => Promise.reject(error);

describe("lazyPage", () => {
  it("shows the fallback while the module loads, then the page with its props", async () => {
    const Page = lazyPage<{ name: string }>(() => Promise.resolve({ default: ({ name }) => <p>hello {name}</p> }), instant);
    await mount(<Page name="ada" />);
    expect(await screen.findByText("hello ada")).toBeInTheDocument();
  });

  it("loads the module once, however often it is preloaded or rendered", async () => {
    const load = vi.fn(() => Promise.resolve({ default: () => <p>page</p> }));
    const Page = lazyPage(load, instant);
    await Page.preload();
    await Page.preload();
    await mount(<Page />);
    expect(await screen.findByText("page")).toBeInTheDocument();
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("renders at once when it was preloaded (a route change from a hover is not a wait)", async () => {
    const Page = lazyPage(() => Promise.resolve({ default: () => <p>page</p> }), instant);
    await Page.preload();
    await mount(<Page />);
    expect(screen.getByText("page")).toBeInTheDocument();
    expect(screen.queryByText("loading")).not.toBeInTheDocument();
  });

  it("turns a missing chunk into an error for the boundary, and marks the build stale", async () => {
    const Page = lazyPage(failing(chunkError()), instant);
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await mount(<Page />);
    expect(await screen.findByText(/failed: Failed to fetch dynamically imported module/)).toBeInTheDocument();
    expect(isStale()).toBe(true);
  });

  it("does not reload the page and does not touch session storage, whatever the library would have done", async () => {
    const reload = vi.fn();
    const original = window.location;
    Object.defineProperty(window, "location", { configurable: true, value: { reload } });
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const Page = lazyPage(failing(chunkError()), instant);
    await mount(<Page />);
    await screen.findByText(/failed:/);
    expect(reload).not.toHaveBeenCalled();
    expect(setItem).not.toHaveBeenCalled();
    Object.defineProperty(window, "location", { configurable: true, value: original });
  });

  it("does not mark the build stale for a module that loaded and then broke", async () => {
    const Page = lazyPage(failing(new Error("boom")), instant);
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await mount(<Page />);
    expect(await screen.findByText("failed: boom")).toBeInTheDocument();
    expect(isStale()).toBe(false);
  });

  it("retries a missing chunk a little before it gives up", async () => {
    const load = vi.fn().mockRejectedValueOnce(chunkError()).mockResolvedValueOnce({ default: () => <p>page</p> });
    const Page = lazyPage(load, instant);
    await mount(<Page />);
    expect(await screen.findByText("page")).toBeInTheDocument();
    expect(load).toHaveBeenCalledTimes(2);
    expect(isStale()).toBe(false);
  });

  it("shows the same error to every render of one failure, so React's second try and the boundary do not start a new load", async () => {
    const load = vi.fn().mockRejectedValue(chunkError());
    const Page = lazyPage(load, instant);
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await mount(<Page />);
    await screen.findByText(/failed:/);
    expect(load).toHaveBeenCalledTimes(3);
  });

  it("can succeed on a later render after a failure (a transient network error is not for ever)", async () => {
    const load = vi.fn().mockRejectedValue(chunkError());
    let clock = 0;
    const Page = lazyPage(load, { ...instant, now: () => clock });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const first = await mount(<Page />);
    await screen.findByText(/failed:/);
    first.unmount();
    clock += 1_500;
    load.mockReset();
    load.mockResolvedValue({ default: () => <p>recovered</p> });
    await mount(<Page />);
    await waitFor(() => expect(screen.getByText("recovered")).toBeInTheDocument());
  });
});
