// What the P10 pages tell the Playwright specs, on `window.__perf`. TEST PAGES, never in the production dist: their routes
// exist only in `vite build --mode perf` (src/router.tsx).

export type PerfLab = {
  /** The page's own account of itself. */
  phase: "loading" | "ready";
  rows?: number;
  /** `performance.now()` when the first `n` key moved the diff. */
  firstN?: number;
  /** `performance.now()` when the diff was first on screen with its keyboard. */
  diffReady?: number;
};

declare global {
  interface Window {
    __perf?: PerfLab;
    __perfControl?: { dropCompare: (alsoEtag: boolean) => void };
  }
}

export function setLab(patch: Partial<PerfLab>): void {
  window.__perf = { phase: "loading", ...window.__perf, ...patch };
}

export function resetLab(): void {
  window.__perf = { phase: "loading" };
}
