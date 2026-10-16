// Vitest setup for the jsdom project (components and features). Nothing here touches the network.
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
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

// Radix pointer handling asks for these; jsdom does not implement them.
Element.prototype.hasPointerCapture = (): boolean => false;
Element.prototype.setPointerCapture = (): void => {};
Element.prototype.releasePointerCapture = (): void => {};
