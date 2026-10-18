import { lazy, Suspense } from "react";
import { Link, useRouterState } from "@tanstack/react-router";
import { ApiProblem } from "@/api/problem";
import type { Role } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { ErrorState } from "@/components/app/ErrorState";
import { NewVersionScreen } from "@/components/app/NewVersion";
import { PageSkeleton } from "@/components/app/PageSkeleton";
import { PermissionState } from "@/components/app/PermissionState";
import { RequestId } from "@/components/app/RequestId";
import { Search } from "@/icons";
import { isChunkLoadError } from "@/lib/deploy/retry-import";
import { routeById } from "./table";

// The screens every route can fall back to: loading, not found, no permission, an error. They are the shared states
// (src/components/app) in the places a route needs them, with the h1 that a route's screen must have (focus goes to it after
// navigation). The app shell provides the <main>; these render into it. A permission is a screen, never a redirect, and an
// error shows a title and a request id and nothing else.

export function RouteLoading() {
  return <PageSkeleton label="Loading" />;
}

const GO_HOME = (
  <Link to="/swimlanes" className="text-sm text-link underline">
    Go to the swimlanes
  </Link>
);

export function NotFoundScreen({ requestId }: { requestId?: string }) {
  return (
    <EmptyState icon={Search} as="h1" title="Page not found" action={GO_HOME} className="pt-16">
      There is no page at this address.
      {requestId ? <RequestId id={requestId} className="mt-2 flex justify-center" /> : null}
    </EmptyState>
  );
}

/** What a screen is called in "<Screen> needs the Admin role": the area for the Admin screens, the route's own title for the rest. */
function subjectOf(routeId: string | undefined): string {
  if (routeId === undefined) return "This screen";
  if (routeId.startsWith("admin-")) return "Admin";
  return routeById(routeId).title;
}

// The one next step of the permission state: ask an Admin for a higher role. It is the access feature's dialog, which this first
// chunk of the app does not carry: it loads when a permission screen is shown, and a person with no role has nothing to ask.
const RoleChangeAction = lazy(() => import("@/features/access/RoleChangeAction"));

export function PermissionScreen({ need, have }: { need: Role | null; have: Role | null }) {
  const routeId = useRouterState({ select: (s) => s.matches.at(-1)?.staticData.routeId });
  return (
    <div className="flex flex-col items-center gap-3 pt-16">
      <PermissionState
        what={subjectOf(routeId)}
        need={need}
        have={have}
        as="h1"
        {...(have === null
          ? {}
          : {
              action: (
                <Suspense fallback={null}>
                  <RoleChangeAction need={need} />
                </Suspense>
              ),
            })}
      />
      {GO_HOME}
    </div>
  );
}

/**
 * An error the router or a query ended in. Shows the problem's title and its request id, and never the message of an
 * error that is not a problem (it could hold anything), nor `detail` (only screens that know a code is safe show it).
 */
export function ErrorScreen({ error }: { error?: unknown }) {
  // A screen whose chunk is gone is not an error of the page: the build is old (a deploy replaced its files).
  if (isChunkLoadError(error)) return <NewVersionScreen />;
  const problem = error instanceof ApiProblem ? error : undefined;
  return <ErrorState as="h1" title="Something went wrong" {...(problem ? { problem } : {})} action={GO_HOME} className="pt-16">{problem ? undefined : "The page could not be shown."}</ErrorState>;
}
