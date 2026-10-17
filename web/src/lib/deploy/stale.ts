import { useSyncExternalStore } from "react";

// "A new version is available." After a deploy the hub serves new files with new hashed names and the old ones are gone
// (the image holds one build). A tab that was open before the deploy learns it the first time it asks for a chunk the new
// build no longer has: Vite then fires `vite:preloadError` on window. The flag below records that, for the banner.
//
// The listener does not call `preventDefault`: the import that failed must still fail, so the router shows its error screen
// (which says the same thing) instead of rendering with a screen missing. Nothing here reloads the page by itself: a reload
// would throw away an edit that has not been saved.

let stale = false;
const listeners = new Set<() => void>();
let installs = 0;

function onPreloadError(): void {
  if (stale) return;
  stale = true;
  for (const listener of [...listeners]) listener();
}

/** Listen for a failed preload. Installing twice listens once. Returns the function that takes the listener off. */
export function installStaleChunkListener(): () => void {
  if (installs === 0) window.addEventListener("vite:preloadError", onPreloadError);
  installs += 1;
  let removed = false;
  return () => {
    if (removed) return;
    removed = true;
    installs -= 1;
    if (installs === 0) window.removeEventListener("vite:preloadError", onPreloadError);
  };
}

export const isStale = (): boolean => stale;

/** Mark the build stale from code that saw a chunk fail some other way (a route's import threw). */
export function markStale(): void {
  onPreloadError();
}

// The full-page "new version" screen and the banner say the same thing with the same button; when the screen is on, the
// banner steps aside.
const screens = new Set<symbol>();
const notify = (): void => {
  for (const listener of [...listeners]) listener();
};

/** The "new version" screen is on screen. Returns the function that says it is gone. */
export function claimNewVersionScreen(): () => void {
  const claim = Symbol("new-version-screen");
  screens.add(claim);
  notify();
  return () => {
    // Releasing twice, or after a reset, does nothing.
    if (screens.delete(claim)) notify();
  };
}

const screenShown = (): boolean => screens.size > 0;

const subscribe = (listener: () => void): (() => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

export function useStale(): boolean {
  return useSyncExternalStore(subscribe, isStale, () => false);
}

/** True when the full-page "new version" screen is showing. */
export function useNewVersionScreenShown(): boolean {
  return useSyncExternalStore(subscribe, screenShown, () => false);
}

/** For tests. */
export function resetStaleForTests(): void {
  stale = false;
  screens.clear();
  notify();
}
