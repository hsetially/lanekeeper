import { useEffect } from "react";
import type { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";

// The editor's idle warm-up (P10). Edit mode opens Monaco, a lazy chunk of a few megabytes (about 1 MB gzipped). For an
// Editor and above, once the file view is interactive, the editor's files are FETCHED while the browser is idle, so that the
// HTTP cache has them when Edit mode asks (the hub marks /assets/* immutable).
//
// FETCHED, never imported: evaluating Monaco is the expensive part, and it installs global patches (src/editor/
// global-patches.ts). A `fetch` is a few small tasks on the main thread; an `import()` or a modulepreload link would parse
// and compile the chunk on it. Each file is read to the end and dropped piece by piece, so nothing is held in memory.
//
// Where the file names come from: the build writes them into a <meta> element of index.html (scripts/build/chunk-hints.mjs),
// because the page cannot know the hashed names. The page treats that as untrusted input all the same: only the app's own
// /assets/ scripts and styles are fetched, at most eight, each once.

export const WARM_META_NAME = "lk-warm-editor";
const MAX_FILES = 8;
const SAFE = /^\/assets\/[A-Za-z0-9._-]+\.(js|css)$/;

export type WarmResult = "warmed" | "skipped" | "failed";

/** The files to fetch, from the meta element of `doc`. Empty when the build wrote none. */
export function warmFiles(doc: Document = document): string[] {
  const content = doc.querySelector(`meta[name="${WARM_META_NAME}"]`)?.getAttribute("content") ?? "";
  const out: string[] = [];
  for (const file of content.split(" ")) {
    if (out.length >= MAX_FILES) break;
    if (SAFE.test(file) && !file.includes("..") && !out.includes(file)) out.push(file);
  }
  return out;
}

type Gate = { role: Role | null; interactive: boolean; saveData: boolean; hidden: boolean };

/** Warm the editor only for a person who can edit, on a page that is up, in a visible tab, who did not ask to save data. */
export function shouldWarm({ role, interactive, saveData, hidden }: Gate): boolean {
  return interactive && !saveData && !hidden && roleAtLeast(role, "editor");
}

let done = false;
export function resetWarmForTests(): void {
  done = false;
}

type Fetcher = (url: string, init: RequestInit & { priority?: "low" | "high" | "auto" }) => Promise<Response>;

/** Read a body to the end and keep none of it. */
async function drain(response: Response, signal: AbortSignal | undefined): Promise<void> {
  const reader = response.body?.getReader();
  if (!reader) return;
  while (!signal?.aborted) {
    const { done: finished } = await reader.read();
    if (finished) return;
  }
  await reader.cancel();
}

export async function warmEditor({ doc = document, fetcher = (url, init) => fetch(url, init), signal }: { doc?: Document; fetcher?: Fetcher; signal?: AbortSignal } = {}): Promise<WarmResult> {
  if (done) return "skipped";
  const files = warmFiles(doc);
  if (files.length === 0) return "skipped";
  performance.mark("lk:editor-warm:start");
  try {
    for (const file of files) {
      if (signal?.aborted) return "skipped";
      const response = await fetcher(file, { credentials: "same-origin", priority: "low", ...(signal ? { signal } : {}) });
      if (!response.ok) return "failed";
      await drain(response, signal);
    }
  } catch {
    // Edit mode fetches what it needs itself; a failed warm-up costs nothing but the head start.
    return "failed";
  }
  if (signal?.aborted) return "skipped";
  done = true;
  performance.mark("lk:editor-warm:done");
  return "warmed";
}

const IDLE_TIMEOUT_MS = 5000;

function scheduleIdle(run: () => void): () => void {
  if (typeof requestIdleCallback === "function") {
    const handle = requestIdleCallback(run, { timeout: IDLE_TIMEOUT_MS });
    return () => {
      if (typeof cancelIdleCallback === "function") cancelIdleCallback(handle);
    };
  }
  const handle = setTimeout(run, 1000);
  return () => clearTimeout(handle);
}

type HookOptions = { doc?: Document; fetcher?: Fetcher; schedule?: (run: () => void) => () => void };

/** Call from the file view with `interactive` true once it can be used: warms the editor for an Editor and above. */
export function useEditorWarmup(interactive: boolean, { doc, fetcher, schedule = scheduleIdle }: HookOptions = {}): void {
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  useEffect(() => {
    const saveData = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection?.saveData === true;
    if (!shouldWarm({ role, interactive, saveData, hidden: document.visibilityState === "hidden" })) return undefined;
    const controller = new AbortController();
    const cancel = schedule(() => {
      void warmEditor({ ...(doc ? { doc } : {}), ...(fetcher ? { fetcher } : {}), signal: controller.signal });
    });
    return () => {
      cancel();
      controller.abort();
    };
  }, [role, interactive, schedule, doc, fetcher]);
}
