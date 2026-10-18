import { useState, type ReactNode } from "react";
import type { ApiProblem } from "@/api/problem";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { supportDetails } from "@/components/app/support-details";
import { Button } from "@/components/ui/button";
import { TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";

/**
 * The design's error state (States index, Error): what failed, the problem's title, one line of what to do, the request id with
 * a copy button, and Retry as the only action. Used by a screen's query boundary and by the router's error screen.
 *
 * S21: what the server wrote in `detail` is shown only for a code the screen named in `detailKinds` as safe to show (a
 * validation message it knows). By default nothing but the title and the request id leaves the problem, and the message of an
 * error that is not a problem is never shown at all.
 */
export function ErrorState({
  title,
  problem,
  children,
  detailKinds,
  onRetry,
  action,
  as: Heading = "h2",
  className,
}: {
  /** What failed, for example "Couldn't load swimlanes". */
  title: string;
  problem?: ApiProblem | undefined;
  /** One line: what happened and what to do next. */
  children?: ReactNode;
  /** The problem kinds whose `detail` is safe to show. */
  detailKinds?: readonly string[];
  /** Offer Retry. Leave it out when trying again cannot change the answer (a 403, a 404). */
  onRetry?: () => void;
  /** Another action beside Retry, such as a link to the swimlanes. */
  action?: ReactNode;
  as?: "h1" | "h2";
  className?: string;
}) {
  const showDetail = problem?.detail !== undefined && detailKinds?.includes(problem.kind) === true;
  // The time the error was shown, for the details a person can copy for support. A problem with no response (status 0: the network,
  // or only a request id from the address) has nothing to describe.
  const [shownAt] = useState(() => Date.now());
  const details = problem?.requestId !== undefined && problem.status > 0 ? supportDetails(problem, shownAt) : undefined;
  return (
    <div role="alert" data-slot="error-state" className={cn("flex flex-col items-center gap-3 px-6 py-10 text-center", className)}>
      <TriangleAlert aria-hidden="true" className="size-6 text-danger-fg" />
      <Heading className="text-base font-semibold">{title}</Heading>
      {problem ? <p className="text-sm font-medium">{problem.title}</p> : null}
      {children ? <p className="max-w-prose text-sm text-muted-foreground">{children}</p> : null}
      {showDetail && problem.detail ? <p className="max-w-prose text-sm text-muted-foreground"><SafeText text={problem.detail} /></p> : null}
      {problem?.requestId ? <RequestId id={problem.requestId} {...(details === undefined ? {} : { details })} /> : null}
      {onRetry || action ? (
        <div className="flex flex-wrap items-center justify-center gap-2">
          {onRetry ? <Button onClick={onRetry}>Retry</Button> : null}
          {action}
        </div>
      ) : null}
    </div>
  );
}
