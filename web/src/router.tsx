import type { QueryClient } from "@tanstack/react-query";
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  lazyRouteComponent,
  notFound,
  Outlet,
  redirect,
  type RouterHistory,
} from "@tanstack/react-router";
import { SessionProvider, RouteGate } from "@/auth/session";
import { Frame } from "@/routes/Frame";
import { Landing } from "@/routes/Landing";
import { SignIn } from "@/routes/SignIn";
import { ErrorScreen, NotFoundScreen, RoutePlaceholder } from "@/routes/screens";
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

function RootLayout() {
  return (
    <SessionProvider>
      <Frame />
      <Outlet />
    </SessionProvider>
  );
}

/** `/swimlanes/:id` becomes `/swimlanes/$id`. */
const toTanstack = (path: string): string => path.replace(/:([A-Za-z]+)/g, "$$$1");

function fillParams(path: string, params: Readonly<Record<string, string>>): string {
  return path.replace(/:([A-Za-z]+)/g, (_m, name: string) => encodeURIComponent(params[name] ?? ""));
}

function screenFor(def: RouteDef) {
  if (def.id === "signin") return SignIn;
  return function Screen() {
    return <RoutePlaceholder def={def} />;
  };
}

export function createAppRouter({ queryClient, history }: { queryClient: QueryClient; history?: RouterHistory }) {
  const root = createRootRouteWithContext<RouterContext>()({
    // A child's search is its parent's merged with its own, so without this every key in the URL would reach every route.
    validateSearch: () => ({}),
    component: RootLayout,
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
    const Screen = screenFor(def);
    return createRoute({
      getParentRoute: () => root,
      path: toTanstack(def.path),
      staticData: { access: def.access, title: def.title, routeId: def.id },
      validateSearch: pickSearch(def.search ?? []),
      beforeLoad: ({ params }: { params: Record<string, string> }) => {
        for (const name of paramNames(def.path)) {
          const check = paramCheck(def, name);
          const value = params[name];
          if (value === undefined || (check && !check(value))) throw notFound();
        }
        if (def.redirect) {
          const target = fillParams(def.redirect.to, params);
          throw redirect({ href: target });
        }
      },
      component: () => (
        <RouteGate access={def.access}>
          <Screen />
        </RouteGate>
      ),
    });
  });

  // Test-only pages (the overlay primitives and the editor), for the CSP and performance runs. They exist only in
  // `vite build --mode perf`: in a production build `import.meta.env.MODE` is the string "production", the branch is dead
  // code and no chunk is emitted (scripts/dist-check.mjs fails a production dist that mentions /testpages/). They are routes
  // of the app, not separate HTML files, because the hub puts the nonce only into index.html.
  const lab =
    import.meta.env.MODE === "perf"
      ? [
          createRoute({
            getParentRoute: () => root,
            path: "/testpages/primitives",
            staticData: { access: "public", title: "Primitives", routeId: "lab-primitives" },
            component: lazyRouteComponent(() => import("@/testpages/primitives"), "PrimitivesPage"),
          }),
          createRoute({
            getParentRoute: () => root,
            path: "/testpages/editor",
            staticData: { access: "public", title: "Editor", routeId: "lab-editor" },
            component: lazyRouteComponent(() => import("@/testpages/EditorLab"), "EditorLab"),
          }),
        ]
      : [];

  const tree = root.addChildren([index, ...routes, ...lab]);
  return createRouter({
    routeTree: tree,
    context: { queryClient },
    ...(history ? { history } : {}),
    parseSearch: parseAppSearch,
    stringifySearch,
    defaultPreload: "intent",
    defaultNotFoundComponent: () => <NotFoundScreen />,
    defaultErrorComponent: ({ error }) => <ErrorScreen error={error} />,
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
