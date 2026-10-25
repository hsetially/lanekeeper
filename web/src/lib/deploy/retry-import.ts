// A dynamic import that is retried a little, because the usual cause of a failed one is short: a network blip, or a deploy
// that has not finished. Bounded (rule 5): two retries, 250 ms then 500 ms at most, with full jitter. A deploy that removed
// the old files fails every try, and that is what the "new version" screen is for.
//
// Browsers remember a failed import of the same address, so a retry may fail at once without asking the network again; the
// bound keeps that cheap, and the screen's "Reload" fetches everything fresh.

const MESSAGES = [
  "failed to fetch dynamically imported module",
  "error loading dynamically imported module",
  "importing a module script failed",
  "unable to preload css",
  "loading chunk",
  "loading css chunk",
];

/** True for the error a failed fetch of a chunk throws, in any browser. A module that loaded and then threw is not one. */
export function isChunkLoadError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const message = error.message.toLowerCase();
  return MESSAGES.some((known) => message.includes(known));
}

export const MAX_RETRIES = 2;
const BASE_MS = 250;
const CAP_MS = 1000;

type Options = { sleep?: (ms: number) => Promise<void>; random?: () => number };

export async function retryImport<T>(load: () => Promise<T>, { sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)), random = Math.random }: Options = {}): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await load();
    } catch (error) {
      if (attempt >= MAX_RETRIES || !isChunkLoadError(error)) throw error;
      await sleep(Math.floor(random() * Math.min(CAP_MS, BASE_MS * 2 ** attempt)));
    }
  }
}
