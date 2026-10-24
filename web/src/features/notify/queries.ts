import { getConfigServerRestartOffer, getRestartOffer } from "@/api/generated/endpoints/writes/writes";
import type { NfsPath } from "@/api/generated/model";
import { swimlaneKeys } from "@/lib/query/keys";
import { apiQuery } from "@/lib/query/options";

// The two offers a person confirms before a restart (CG5), both an Operator's read:
//  - what the config-server restart would take away: `offer` is null unless check C11 is open for the swimlane and the cluster
//    report lists exactly one Deployment with the configured name, because the hub does not guess which one is meant;
//  - what a restart of the services that read a file would restart: every service for a file in the root folder, one
//    application's Deployments for a file in its folder.
//
// The hub answers both `no-store`, and a person decides a restart on them, so neither is shown from an old answer or kept
// after the dialog is gone: `staleTime: 0` (asked again whenever a dialog mounts it) and `gcTime: 0`. They sit under the
// swimlane's own key, so the events that refresh the swimlane (`findings_changed` for C11, `pickup_changed`,
// `swimlane_changed`) and the writes that change its services refetch one that is on screen. A screen mounts them only while
// its dialog is open (`enabled`), so no Viewer, and no swimlane nobody asked about, is ever asked.

/** The config-server restart that finding C11 asks for. */
export const configServerRestartOfferQuery = (id: string) =>
  apiQuery({
    queryKey: swimlaneKeys.configServerRestart(id),
    queryFn: ({ signal }) => getConfigServerRestartOffer(id, { signal }),
    staleTime: 0,
    gcTime: 0,
  });

/** The services that a change to `path` calls for restarting ("restart all"). */
export const restartOfferQuery = (id: string, path: NfsPath) =>
  apiQuery({
    queryKey: swimlaneKeys.restartOffer(id, path),
    queryFn: ({ signal }) => getRestartOffer(id, { path }, { signal }),
    staleTime: 0,
    gcTime: 0,
  });
