import { useEffect, useRef, useState } from "react";
import { adminListUsers } from "@/api/generated/endpoints/admin/admin";
import { searchDocs } from "@/api/generated/endpoints/docs/docs";
import { searchSettings } from "@/api/generated/endpoints/search/search";
import type { DocHit, SettingHit, User } from "@/api/generated/model";
import type { PaletteGroupSpec, Remote } from "./model";

// The palette's three searches that go to the server: settings, docs and users. Each waits for the person to stop typing
// (the table's debounce), asks for at most the group's cap, cancels the request it is replacing, and keeps its answers in
// memory for as long as the palette is open and not a moment longer: a search result is not put in the app's query cache,
// where it would outlive the palette and the session's next user would have to be kept out of it.

const MAX_MEMORY = 32;

export const fetchSettings = (q: string, limit: number, signal: AbortSignal): Promise<SettingHit[]> => searchSettings({ q, limit }, { signal }).then((page) => page.items);
export const fetchDocs = (q: string, limit: number, signal: AbortSignal): Promise<DocHit[]> => searchDocs({ q, limit }, { signal }).then((found) => found.hits);
export const fetchUsers = (q: string, limit: number, signal: AbortSignal): Promise<User[]> => adminListUsers({ q, limit }, { signal }).then((page) => page.items);

function useDebounced(value: string, ms: number): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    if (ms <= 0) {
      setDebounced(value);
      return undefined;
    }
    const timer = setTimeout(() => {
      setDebounced(value);
    }, ms);
    return () => {
      clearTimeout(timer);
    };
  }, [value, ms]);
  return ms <= 0 ? value : debounced;
}

/**
 * One remote group's results for the query. `enabled` is false for a role that does not get the group (nothing is asked).
 * While the person is still typing the previous answer stays up, so the list does not flicker between keystrokes.
 */
export function useRemoteGroup<T>(query: string, group: PaletteGroupSpec, enabled: boolean, fetcher: (q: string, limit: number, signal: AbortSignal) => Promise<T[]>): Remote<T> {
  const text = useDebounced(query.trim(), group.debounceMs);
  const active = enabled && text.length >= group.minQuery;
  const [state, setState] = useState<Remote<T>>({ status: "idle", items: [] });
  const memory = useRef(new Map<string, readonly T[]>());
  const latest = useRef(fetcher);
  useEffect(() => {
    latest.current = fetcher;
  });

  useEffect(() => {
    if (!active) {
      setState((s) => (s.status === "idle" ? s : { status: "idle", items: [] }));
      return undefined;
    }
    const known = memory.current.get(text);
    if (known) {
      setState({ status: "ready", items: known });
      return undefined;
    }
    const controller = new AbortController();
    setState((s) => ({ status: "loading", items: s.items }));
    latest
      .current(text, group.cap, controller.signal)
      .then((items) => {
        if (controller.signal.aborted) return;
        if (memory.current.size >= MAX_MEMORY) {
          const oldest = memory.current.keys().next().value;
          if (oldest !== undefined) memory.current.delete(oldest);
        }
        memory.current.set(text, items);
        setState({ status: "ready", items });
      })
      .catch(() => {
        if (!controller.signal.aborted) setState({ status: "error", items: [] });
      });
    return () => {
      controller.abort();
    };
  }, [active, text, group.cap]);

  // Forget everything when the palette closes (this component unmounts with it).
  useEffect(() => {
    const kept = memory.current;
    return () => {
      kept.clear();
    };
  }, []);

  return active ? state : { status: "idle", items: [] };
}
