import { createElement, use, type ComponentType, type ReactNode } from "react";
import { isChunkLoadError, retryImport } from "./retry-import";
import { markStale } from "./stale";

// A route's screen, loaded when the route is first used (route-level code splitting, P10).
//
// This replaces TanStack Router's `lazyRouteComponent`, which reloads the whole page, once, when a chunk is missing: a
// reload throws away whatever the person has typed and has not saved, and it keeps its "reloaded already" flag in
// sessionStorage, which the app keeps to `lib/prefs`. Here a missing chunk is an error like any other: the router's error
// screen says "A new version is available", the banner says it too, and "Reload" waits for the dirty registry to be empty.
//
// It keeps what the router needs of a lazy component: `.preload()`, which the router calls on hover and focus
// (`defaultPreload: "intent"`) and the idle prefetch calls for the sidebar's destinations, so that the click that follows is
// a route change that waits for nothing. `preload()` never rejects: a failure is kept and thrown when the page renders.
//
// A failure is kept for one second, not for ever. React renders a component that threw once more at once, and the error
// boundary renders it again when it is told to show the error; both must see the same error, or the page would start a new
// load and hang in its loading state. After that second a new render asks the network again, so a failure that was only a
// blip is not final.

export type LazyPage<P extends object> = ((props: P) => ReactNode) & { preload: () => Promise<void> };

type Options = Parameters<typeof retryImport>[1] & { now?: () => number; failureMs?: number };

const FAILURE_MS = 1000;

export function lazyPage<P extends object = object>(load: () => Promise<{ default: ComponentType<P> }>, options: Options = {}): LazyPage<P> {
  const { now = () => Date.now(), failureMs = FAILURE_MS, ...retry } = options;
  let loading: Promise<void> | undefined;
  let component: ComponentType<P> | undefined;
  let failure: { error: unknown; until: number } | undefined;

  const start = (): Promise<void> => {
    if (!loading) {
      failure = undefined;
      loading = retryImport(load, retry)
        .then((module) => {
          component = module.default;
        })
        .catch((error: unknown) => {
          // Forget the promise, so that a later render asks again; keep the error, so that this render fails with it.
          loading = undefined;
          failure = { error, until: now() + failureMs };
          if (isChunkLoadError(error)) markStale();
        });
    }
    return loading;
  };

  function Page(props: P) {
    if (failure && now() < failure.until) throw failure.error instanceof Error ? failure.error : new Error("The page could not be loaded");
    if (!component) use(start());
    // `use` has just waited, or it has thrown the failure at the next render.
    return component ? createElement(component, props) : null;
  }
  Page.preload = start;
  return Page;
}
