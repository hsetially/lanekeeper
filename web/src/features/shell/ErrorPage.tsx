import { ErrorState } from "@/components/app/ErrorState";
import { ApiProblem } from "@/api/problem";
import { Link } from "@tanstack/react-router";
import type { PageProps } from "@/routes/props";

/** `/error?req=<request id>`: where a failure the app could not show in place sends a person. Only the request id is known. */
export default function ErrorPage({ search }: PageProps<"error">) {
  const problem = search.req ? new ApiProblem({ status: 0, kind: "unknown", title: "The request failed", requestId: search.req }) : undefined;
  return (
    <ErrorState
      as="h1"
      title="Something went wrong"
      {...(problem ? { problem } : {})}
      action={
        <Link to="/swimlanes" className="text-sm text-link underline">
          Go to the swimlanes
        </Link>
      }
      className="pt-16"
    >
      Send the request ID to the platform team if this keeps happening.
    </ErrorState>
  );
}
