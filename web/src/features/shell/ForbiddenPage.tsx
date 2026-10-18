import { Link } from "@tanstack/react-router";
import { EmptyState } from "@/components/app/EmptyState";
import { RequestId } from "@/components/app/RequestId";
import { Lock } from "@/icons";
import type { PageProps } from "@/routes/props";

/** `/403?req=<request id>`: the server refused a request and the app has no better place to say so. */
export default function ForbiddenPage({ search }: PageProps<"forbidden">) {
  return (
    <EmptyState
      icon={Lock}
      as="h1"
      title="You do not have access"
      className="pt-16"
      action={
        <Link to="/swimlanes" className="text-sm text-link underline">
          Go to the swimlanes
        </Link>
      }
    >
      The server refused this request. If you think you should have access, ask an Admin, and send them the request ID.
      {search.req ? <RequestId id={search.req} className="mt-2 flex justify-center" /> : null}
    </EmptyState>
  );
}
