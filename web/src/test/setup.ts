// Vitest setup for the jsdom project (components and features). Nothing here touches the network.
import "@testing-library/jest-dom/vitest";
import { cleanup, configure } from "@testing-library/react";
import { afterEach } from "vitest";

// A screen is a lazy chunk, and the first test that opens one evaluates the screen's whole module graph in the test's own thread
// (a real screen pulls in hundreds of modules), so on a busy machine the first wait can take well over Testing Library's default
// second. The wait ends as soon as the thing is there; this only says how long to give up after. A test that needs more says so.
configure({ asyncUtilTimeout: 5_000 });

// Idle work: the shell loads the palette and the sidebar's screens when the browser has nothing else to do. jsdom has no idle
// callback, so the app falls back to a timer, and a timer that fires in the middle of a test loads dozens of modules in the
// background: it takes the event loop from the test (a wait that ends in a second on an idle machine ends in five on a busy one),
// and it can still be loading when the file ends, which Vitest reports as an EnvironmentTeardownError. So here the browser is
// never idle. A test that is about idle work installs its own callback (routes/prefetch.test.tsx).
Object.defineProperty(globalThis, "requestIdleCallback", { configurable: true, writable: true, value: (): number => 0 });
Object.defineProperty(globalThis, "cancelIdleCallback", { configurable: true, writable: true, value: (): void => undefined });

// Radix's FocusScope hands focus back from a `setTimeout(..., 0)` that it queues when a dialog, popover or menu unmounts. If that
// was the last test of a file, the timer could fire after the jsdom environment was torn down, where `CustomEvent` is no longer
// jsdom's, and Vitest reported `TypeError: Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'`
// as an unhandled error: the run failed with every test passing (CI, 2026-10-26, DeleteDialog.test.tsx). Waiting one turn of the
// event loop after the unmount lets the timers that it queued fire while the window is still there (after-each-timers.test.tsx).
afterEach(async () => {
  cleanup();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
});

// jsdom has no layout. Components that measure (the virtualiser, Radix popper) read these.
// Default box: 1024x768, overridable per element with data-test-width and data-test-height.
const DEFAULT_WIDTH = 1024;
const DEFAULT_HEIGHT = 768;

function testSize(el: Element, attr: "testWidth" | "testHeight", fallback: number): number {
  const raw = el instanceof HTMLElement ? el.dataset[attr] : undefined;
  const parsed = raw === undefined ? Number.NaN : Number(raw);
  return Number.isFinite(parsed) ? parsed : fallback;
}

Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
  configurable: true,
  value(this: HTMLElement): DOMRect {
    const width = testSize(this, "testWidth", DEFAULT_WIDTH);
    const height = testSize(this, "testHeight", DEFAULT_HEIGHT);
    return {
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: width,
      bottom: height,
      width,
      height,
      toJSON: () => ({}),
    } satisfies DOMRect;
  },
});
for (const [prop, attr, fallback] of [
  ["offsetWidth", "testWidth", DEFAULT_WIDTH],
  ["offsetHeight", "testHeight", DEFAULT_HEIGHT],
  ["clientWidth", "testWidth", DEFAULT_WIDTH],
  ["clientHeight", "testHeight", DEFAULT_HEIGHT],
] as const) {
  Object.defineProperty(HTMLElement.prototype, prop, {
    configurable: true,
    get(this: HTMLElement) {
      return testSize(this, attr, fallback);
    },
  });
}

class ResizeObserverStub implements ResizeObserver {
  constructor(private readonly callback: ResizeObserverCallback) {}
  observe(target: Element): void {
    const rect = target.getBoundingClientRect();
    const entry = {
      target,
      contentRect: rect,
      borderBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }],
      contentBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }],
      devicePixelContentBoxSize: [],
    } satisfies ResizeObserverEntry;
    this.callback([entry], this);
  }
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = ResizeObserverStub;

if (typeof window.matchMedia !== "function") {
  // Light theme, no reduced motion, unless a test replaces it.
  window.matchMedia = (query: string): MediaQueryList => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  });
}

Element.prototype.scrollIntoView = function scrollIntoView(): void {};
// The router scrolls to the top on navigation; jsdom logs "not implemented" for it.
window.scrollTo = (): void => {};

// Radix pointer handling asks for these; jsdom does not implement them.
Element.prototype.hasPointerCapture = (): boolean => false;
Element.prototype.setPointerCapture = (): void => {};
Element.prototype.releasePointerCapture = (): void => {};
