import type { QueryClient } from "@tanstack/react-query";
import type { ComponentType } from "react";
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  notFound,
  Outlet,
  redirect,
  useParams,
  useSearch,
  type RouterHistory,
} from "@tanstack/react-router";
import { SessionProvider, RouteGate } from "@/auth/session";
import { Landing } from "@/routes/Landing";
import { lazyPage } from "@/lib/deploy/lazy-page";
import { AppShell } from "@/features/shell/AppShell";
import { LiveProvider } from "@/live/LiveProvider";
import type { LiveConfig } from "@/live/live";
import { PAGE_LOADERS, type AppPageProps, type PageLoader, type PageLoaders } from "@/routes/pages";
import { useIdlePrefetch } from "@/routes/prefetch";
import { redirectHref } from "@/routes/redirect";
import { ErrorScreen, NotFoundScreen, RouteLoading } from "@/routes/screens";
import { parseAppSearch, pickSearch, stringifySearch } from "@/routes/search";
import { paramCheck, paramNames, ROUTES, type RouteAccess, type RouteDef } from "@/routes/table";

// The router, built from the route table (src/routes/table.ts) and nothing else. Each route carries its access
// (`public` or a minimum role) as static data; the session provider reads it to decide whether to ask who the user is, and
// the gate around each screen checks the role. A screen is a placeholder until T4 replaces it with a lazy import of the
// feature's page. Search parameters are strings only (src/routes/search.ts); path parameters are checked here, before any
// screen renders or any request is built from them.

declare module "@tanstack/react-router" {
  interface StaticDataRouteOption {
    access?: RouteAccess;
    title?: string;
    routeId?: string;
  }
}

export type RouterContext = { queryClient: QueryClient };

/** Loads the sidebar's destinations while the page is idle. A component of its own because it reads the session. */
function IdlePrefetch() {
  useIdlePrefetch();
  return null;
}

/**
 * The session, then the live stream (one for the tab, only for a signed-in user with a role; see src/live/LiveProvider.tsx),
 * then the chrome and the screen. The stream sits inside the session because it needs to know who is signed in, and it
 * provides the write gate because only it knows whether live updates are up.
 */
function RootLayout({ live }: { live: LiveConfig | undefined }) {
  return (
    <SessionProvider>
      <LiveProvider config={live}>
        <IdlePrefetch />
        <AppShell>
          <Outlet />
        </AppShell>
      </LiveProvider>
    </SessionProvider>
  );
}

/** `/swimlanes/:id` becomes `/swimlanes/$id`. */
const toTanstack = (path: string): string => path.replace(/:([A-Za-z]+)/g, "$$$1");

/**
 * A page behind the role gate. The page is loaded when the route is first used, and the router preloads it on hover and focus
 * (`defaultPreload: "intent"`) by calling `.preload` on the route's component, which is why the gate is wrapped here and the
 * page's `preload` is put on the wrapper.
 */
function gated(access: RouteAccess, load: PageLoader) {
  const Screen = lazyPage<AppPageProps>(load);
  const Gated = () => {
    // The page gets the path parameters (already checked by `beforeLoad`) and the search keys `validateSearch` kept, as props:
    // it never reads the URL by itself.
    const params = useParams({ strict: false }) as unknown as Record<string, string>;
    const search = useSearch({ strict: false }) as unknown as Record<string, string | undefined>;
    return (
      <RouteGate access={access}>
        <Screen params={params} search={search} />
      </RouteGate>
    );
  };
  Gated.preload = Screen.preload;
  return Gated;
}

/** The component of a route: its page behind the role gate. A route that only redirects shows nothing. */
function componentFor(def: RouteDef, pages: PageLoaders) {
  const load = pages[def.id];
  return load ? gated(def.access, load) : () => null;
}

export function createAppRouter({
  queryClient,
  history,
  pages = PAGE_LOADERS,
  live,
}: {
  queryClient: QueryClient;
  history?: RouterHistory;
  pages?: PageLoaders;
  /** The live layer's settings. Production passes none; a test of something else passes `{ enabled: false }`. */
  live?: LiveConfig;
}) {
  const root = createRootRouteWithContext<RouterContext>()({
    // A child's search is its parent's merged with its own, so without this every key in the URL would reach every route.
    validateSearch: () => ({}),
    component: () => <RootLayout live={live} />,
    notFoundComponent: () => <NotFoundScreen />,
    errorComponent: ({ error }) => <ErrorScreen error={error} />,
  });

  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    staticData: { access: "viewer", title: "Lanekeeper", routeId: "landing" },
    component: () => (
      <RouteGate access="viewer">
        <Landing />
      </RouteGate>
    ),
  });

  const routes = ROUTES.map((def) => {
    return createRoute({
      getParentRoute: () => root,
      path: toTanstack(def.path),
      staticData: { access: def.access, title: def.title, routeId: def.id },
      validateSearch: pickSearch(def.search ?? []),
      beforeLoad: ({ params, search }: { params: Record<string, string>; search: Record<string, string | undefined> }) => {
        for (const name of paramNames(def.path)) {
          const check = paramCheck(def, name);
          const value = params[name];
          if (value === undefined || (check && !check(value))) throw notFound();
        }
        if (def.redirect) throw redirect({ href: redirectHref(def.redirect, params, search) });
      },
      component: componentFor(def, pages),
    });
  });

  // Test-only pages (the overlay primitives, the editor and the P10 pages), for the CSP and performance runs. They exist only
  // in `vite build --mode perf`: in a production build `import.meta.env.MODE` is the string "production", the branch is dead
  // code and no chunk is emitted (scripts/dist-check.mjs fails a production dist that mentions /testpages/). They are routes
  // of the app, not separate HTML files, because the hub puts the nonce only into index.html.
  const lab =
    import.meta.env.MODE === "perf"
      ? [
          createRoute({
            getParentRoute: () => root,
            path: "/testpages/primitives",
            staticData: { access: "public", title: "Primitives", routeId: "lab-primitives" },
            component: lazyPage(() => import("@/testpages/primitives").then((m) => ({ default: m.PrimitivesPage }))),
          }),
          createRoute({
            getParentRoute: () => root,
            path: "/testpages/editor",
            staticData: { access: "public", title: "Editor", routeId: "lab-editor" },
            component: lazyPage(() => import("@/testpages/EditorLab").then((m) => ({ default: m.EditorLab }))),
          }),
          // The P10 pages: a 5,000-row grid, a 6,000-line diff, and three screens to change routes between. Signed-in routes
          // like the real ones, so the session and the role gate are in the measurement.
          createRoute({
            getParentRoute: () => root,
            path: "/testpages/perf/grid",
            staticData: { access: "viewer", title: "Grid", routeId: "lab-perf-grid" },
            component: gated("viewer", () => import("@/testpages/perf/Grid").then((m) => ({ default: m.PerfGrid }))),
          }),
          createRoute({
            getParentRoute: () => root,
            path: "/testpages/perf/diff",
            staticData: { access: "viewer", title: "Diff", routeId: "lab-perf-diff" },
            component: gated("viewer", () => import("@/testpages/perf/Diff").then((m) => ({ default: m.PerfDiff }))),
          }),
          (() => {
            const nav = createRoute({
              getParentRoute: () => root,
              path: "/testpages/perf/nav",
              staticData: { access: "viewer", title: "Pages", routeId: "lab-perf-nav" },
              component: gated("viewer", () => import("@/testpages/perf/Nav").then((m) => ({ default: m.PerfNavLayout }))),
            });
            const page = (id: "a" | "b" | "c", load: () => Promise<{ default: ComponentType }>) =>
              createRoute({ getParentRoute: () => nav, path: id, staticData: { access: "viewer", title: `Route ${id.toUpperCase()}`, routeId: `lab-perf-${id}` }, component: lazyPage(load) });
            return nav.addChildren([
              page("a", () => import("@/testpages/perf/RouteA")),
              page("b", () => import("@/testpages/perf/RouteB")),
              page("c", () => import("@/testpages/perf/RouteC")),
            ]);
          })(),
        ]
      : [];

  const tree = root.addChildren([index, ...routes, ...lab]);
  return createRouter({
    routeTree: tree,
    context: { queryClient },
    ...(history ? { history } : {}),
    parseSearch: parseAppSearch,
    stringifySearch,
    // A link that is hovered or focused loads its screen, so the click that follows is a route change that waits for nothing.
    defaultPreload: "intent",
    defaultPendingComponent: RouteLoading,
    defaultNotFoundComponent: () => <NotFoundScreen />,
    defaultErrorComponent: ({ error }) => <ErrorScreen error={error} />,
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
