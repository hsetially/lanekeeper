// A wait with a bound. AGENTS.md rule 5 (everything is bounded): the Lighthouse runner waits for a browser, for the debugging
// protocol and for whole runs, and any of them can take forever when the machine is not the one it was written on. The first
// run on GitHub proved it: nothing was logged for 38 minutes. A wait that goes through `within` ends, with a message that says
// what it was waiting for.

/**
 * @template T
 * @param {Promise<T>} promise the work
 * @param {number} ms the bound
 * @param {string | (() => string)} message the error when the bound is reached: say what was waited for, and for how long. A
 *   function is called when the bound is reached, so that the message can carry what is known only by then (what the browser printed).
 * @returns {Promise<T>} what the work answered, or its failure, whichever comes first; the timer never outlives the wait
 */
export function within(promise, ms, message) {
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let timer;
  const bound = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(typeof message === "function" ? message() : message)), ms);
  });
  // The timer ends with the wait, whichever way it ends, so a run that finished is not held open by the bound of its last wait.
  return Promise.race([promise, bound]).finally(() => clearTimeout(timer));
}
