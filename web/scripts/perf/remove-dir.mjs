// Removes a folder that something may still be writing into: the profile of a Chromium that was just killed.
//
// SIGKILL ends the browser's main process and not the helper processes it started (zygote, GPU, renderer, network service).
// They keep creating files in the profile until they notice that the main process is gone, so a single recursive remove can
// find the folder not empty again (ENOTEMPTY) and throw. Waiting is the cure: the helpers stop within a moment, and then the
// removal works. This tries again for a short, bounded time and only for the errors that waiting can cure.
import { rmSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

// ENOTEMPTY and EBUSY: a process still holds or fills the folder. EPERM: the same on file systems that refuse to remove an open
// file. EMFILE and ENFILE: the descriptors ran out for a moment. Anything else (EACCES, a bad path) will not be cured by waiting.
const WAITING_HELPS = new Set(["ENOTEMPTY", "EBUSY", "EPERM", "EMFILE", "ENFILE"]);

/**
 * @param {string} dir
 * @param {{ remove?: (dir: string) => void; attempts?: number; delayMs?: number }} [options] `attempts` tries in all, `delayMs`
 *   apart: by default 30 tries, 100 ms apart, so a folder is given about three seconds to stop being written to.
 * @returns {Promise<void>} resolves when the folder is gone (or never was); rejects with the last error when it is not
 */
export async function removeDir(dir, { remove = (target) => rmSync(target, { recursive: true, force: true }), attempts = 30, delayMs = 100 } = {}) {
  for (let attempt = 1; ; attempt++) {
    try {
      remove(dir);
      return;
    } catch (error) {
      const code = error instanceof Error ? /** @type {NodeJS.ErrnoException} */ (error).code : undefined;
      if (attempt >= attempts || code === undefined || !WAITING_HELPS.has(code)) throw error;
      await sleep(delayMs);
    }
  }
}
