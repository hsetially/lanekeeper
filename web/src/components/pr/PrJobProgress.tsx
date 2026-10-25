import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { PrJob } from "@/api/generated/model";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { prJobQuery } from "@/features/prs/queries";
import { useQueryState } from "@/lib/query/use-query-state";
import { useLiveStatus } from "@/live/LiveProvider";
import { isTerminal } from "./job-steps";
import { PrJobView } from "./PrJobView";

// A pull-request job on screen, kept as fresh as the server (T5, "PR job progress view driven by `pr_job_progress` and
// `getPrJob`, never faking completion").
//
// The data is `getPrJob` (src/features/prs/queries.ts), an EVENT_FED query: `pr_job_progress` names `['pr-job', id]` and the query is
// read again, so what the view shows is always what the SERVER answered, never what an event claimed. It starts from the job the
// `createPr` 202 gave (`initial`, shown at once) and from nothing but the id when a job is found again.
//
//  - The job is read again only on an event. When the live stream is down there is no event, so the view says it may be behind and
//    offers "Check now". There is no polling (the plan: no polling), and a job that is completed or failed is not checked.
//  - A failed read keeps the last state it has and says so, with the request ID and Retry. With nothing to show it is the
//    boundary's error (the problem's title and the request ID).

export function PrJobProgress({ jobId, initial, effect, title }: { jobId: number; initial?: PrJob | undefined; effect?: ReactNode; title?: string | undefined }) {
  const query = useQuery({ ...prJobQuery(jobId), ...(initial === undefined ? {} : { placeholderData: initial }) });
  const state = useQueryState(query);
  const live = useLiveStatus();
  const job = query.data ?? initial;

  if (job === undefined) {
    return (
      <QueryBoundaryView state={state} noun="the pull request job" staleBanner={false}>
        {() => null}
      </QueryBoundaryView>
    );
  }
  const paused = (live.status === "offline" || live.status === "stopped") && !isTerminal(job.state);
  return (
    <PrJobView
      job={job}
      {...(effect === undefined ? {} : { effect })}
      title={title}
      behind={
        paused
          ? {
              onCheck: () => {
                void query.refetch();
              },
              checking: query.isFetching,
            }
          : null
      }
      refreshFailed={
        query.isError
          ? {
              requestId: query.error.requestId,
              onRetry: () => {
                void query.refetch();
              },
            }
          : null
      }
    />
  );
}
