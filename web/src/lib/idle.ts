// Two small browser facts that the app's idle work asks (the route preloads, the editor warm-up, the command palette's chunk). Each was
// written in two or three places; the numbers stay the callers' (a route preload may wait 200 ms where there is no idle callback, the
// editor 1 s), so what changes by caller is an argument, not a copy.

/** Whether the person asked the browser to save data (the Save-Data hint): then nothing is loaded ahead of time. */
export function saveDataOn(): boolean {
  const connection = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection;
  return connection?.saveData === true;
}

export type IdleOptions = {
  /** The longest the browser may wait for an idle moment before it runs the work anyway. */
  timeoutMs: number;
  /** Where the browser has no idle callback (Safari): run after this many milliseconds. */
  fallbackMs: number;
};

/** Run `run` when the browser is idle. Returns the function that cancels it. */
export function scheduleIdle(run: () => void, { timeoutMs, fallbackMs }: IdleOptions): () => void {
  if (typeof requestIdleCallback === "function") {
    const handle = requestIdleCallback(run, { timeout: timeoutMs });
    return () => {
      if (typeof cancelIdleCallback === "function") cancelIdleCallback(handle);
    };
  }
  const handle = setTimeout(run, fallbackMs);
  return () => {
    clearTimeout(handle);
  };
}
