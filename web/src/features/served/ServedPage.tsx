import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { EmptyState } from "@/components/app/EmptyState";
import { FileCode } from "@/icons";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { agentView } from "@/features/swimlanes/agent";
import { useQueryState } from "@/lib/query/use-query-state";
import { AppLink, useNavigateTo } from "@/routes/AppLink";
import type { PageProps } from "@/routes/props";
import { readSelection, sameSelection, servedHref, type ServedSelection } from "./model";
import { servedFileQuery } from "./queries";
import { SelectionForm } from "./SelectionForm";
import { ServedResult } from "./ServedResult";

// A file as the config-server serves it (route `swimlane-served`): the page that the MCP tool `get_served_file` sends people to
// (`LinkTemplates::default().served`, with the keys application, tenant and file; channel is ours). The handoff has no frame for it
// (H2), so it is built from the File view's header and status bar, the Compare toolbar's pickers and the shared query boundary.
//
// The selection is in the ADDRESS and nowhere else, so a link, a reload and Back all mean the same thing, and the form only holds
// what the person is typing. Pressing the button goes to the address of the new selection; pressing it for the selection already
// shown asks the config-server again, because the answer is live and nothing tells this page that it changed (queries.ts).
//
// What is NOT here, and why: the engine's comparison of the served text with the rendered view and with NFS, and the "Rendered"
// toggle on a resource file. The contract has no rendered text and no served comparison (CG8), so there is nothing to compare, and the
// view says so in one line rather than drawing a control that does nothing (design/DEVIATIONS.md).

function Choose({ invalid }: { invalid: boolean }) {
  return (
    <EmptyState icon={FileCode} title={invalid ? "That selection can't be asked for" : "Choose what to ask for"}>
      {invalid
        ? "Fix the fields marked above, then press Show served text."
        : "Give a file, an application and a tenant, then press Show served text. The hub asks the config-server in this swimlane's cluster what it would serve, and shows the answer as it is."}
    </EmptyState>
  );
}

export default function ServedPage({ params, search }: PageProps<"swimlane-served">) {
  const swimlane = params.id;
  const navigate = useNavigateTo();
  const read = useMemo(() => readSelection(search), [search]);

  // The swimlane gives the tenants to choose from and whether its agent is there (the hub asks the config-server through it).
  const detail = useQuery(swimlaneQuery(swimlane));
  const served = useQuery(servedFileQuery(swimlane, read.selection));
  const state = useQueryState(served);

  const show = (selection: ServedSelection): void => {
    if (sameSelection(selection, read.selection)) {
      void served.refetch();
      return;
    }
    navigate(servedHref(swimlane, selection));
  };

  const agent = detail.data === undefined ? null : { online: agentView(detail.data.agent).connected };
  const tenants = detail.data?.tenants ?? [];

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">As served</h1>
          <span className="text-muted-foreground">
            <AppLink route="swimlane" params={{ id: swimlane }} className="font-mono">
              {swimlane}
            </AppLink>{" "}
            · A file as the config-server answers for one application, tenant and channel
          </span>
        </div>
      </div>
      <div className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
        <SelectionForm draft={read.draft} problems={read.problems} tenants={tenants} busy={served.isFetching} onShow={show} />
        {read.selection === null ? (
          <Choose invalid={Object.keys(read.problems).length > 0} />
        ) : (
          <ServedResult swimlane={swimlane} state={state} agent={agent} fetching={served.isFetching} asOf={served.dataUpdatedAt > 0 ? served.dataUpdatedAt : null} />
        )}
      </div>
    </div>
  );
}
