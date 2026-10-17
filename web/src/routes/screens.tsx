import { Link } from "@tanstack/react-router";
import { ApiProblem } from "@/api/problem";
import type { Role } from "@/api/generated/model";
import { RequestId } from "@/components/app/RequestId";
import { Banner } from "@/components/app/Banner";
import { Skeleton } from "@/components/ui/skeleton";
import { roleLabel } from "@/auth/roles";
import { NewVersionScreen } from "@/components/app/NewVersion";
import { isChunkLoadError } from "@/lib/deploy/retry-import";

// The screens every route can fall back to. The designed versions (focus to the h1 after navigation, the handoff's
// permission and error states) are built by the shell slice in T4 and replace these; what stays is the contract: a
// permission screen is a screen, not a redirect, and an error shows a title and a request id and nothing else.

export function RouteLoading() {
  return (
    <main aria-busy="true" aria-label="Loading" className="flex flex-col gap-3 p-6">
      <Skeleton className="h-6 w-56" />
      <Skeleton className="h-4 w-96" />
      <Skeleton className="h-4 w-80" />
    </main>
  );
}

export function NotFoundScreen({ requestId }: { requestId?: string }) {
  return (
    <main className="flex flex-col gap-3 p-6">
      <h1 className="text-lg font-semibold">Page not found</h1>
      <p className="text-muted-foreground">There is no page at this address.</p>
      {requestId ? <RequestId id={requestId} /> : null}
      <Link to="/swimlanes" className="text-link underline">
        Go to the swimlanes
      </Link>
    </main>
  );
}

export function PermissionScreen({ need }: { need: Role | null }) {
  return (
    <main className="flex flex-col gap-3 p-6">
      <h1 className="text-lg font-semibold">You do not have access</h1>
      <p className="text-muted-foreground">
        {need === null ? "You have no role yet. Ask an Admin to grant access." : `This screen needs the ${roleLabel(need)} role. Ask an Admin to grant it.`}
      </p>
      <Link to="/swimlanes" className="text-link underline">
        Go to the swimlanes
      </Link>
    </main>
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
  return (
    <main className="flex flex-col gap-3 p-6">
      <h1 className="text-lg font-semibold">Something went wrong</h1>
      <Banner variant="danger" title={problem?.title ?? "The page could not be shown"} />
      {problem?.requestId ? <RequestId id={problem.requestId} /> : null}
      <Link to="/swimlanes" className="text-link underline">
        Go to the swimlanes
      </Link>
    </main>
  );
}
