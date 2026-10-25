import { FindingKind, PickupState, type SwimlaneDetail } from "@/api/generated/model";
import { Banner } from "@/components/app/Banner";
import { findingListQuery } from "@/features/findings/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { RestartConfigServerButton } from "./RestartConfigServerButton";

// The line on the Services tab that says a config-server restart is needed: finding C11 is open for the swimlane, or a service
// reports that it waits for one (pickup state "Needs config-server restart"). It says which of the two is the case and does not
// claim the other. The finding is read like the Findings tab reads it (one finding is enough), so a Viewer sees the banner too and
// the events that refresh findings (`findings_changed`) refresh it. If the findings cannot be read, the banner stands on what the
// services report and says nothing about the failure: the Findings tab says it, and a missing banner is not a claim that no
// restart is needed. Whether the hub OFFERS a restart is the dialog's to read, when the button is pressed.

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

export function ConfigServerRestartBanner({ lane }: { lane: SwimlaneDetail }) {
  const c11 = useCursorPages({ ...findingListQuery({ swimlane: lane.id, kinds: [FindingKind.config_server_restart_required], limit: 1 }), maxPages: 1 });
  // The hub filters by swimlane and kind; the screen checks both again, so a finding of another kind or swimlane is never taken for C11.
  const open = c11.status === "success" && c11.items.some((found) => found.kind === FindingKind.config_server_restart_required && found.swimlane === lane.id);
  const waiting = lane.services.filter((service) => service.pickup === PickupState.needs_config_server_restart).length;
  if (!open && waiting === 0) return null;
  return (
    <Banner variant="warning" title="Config-server restart needed" action={<RestartConfigServerButton lane={lane} />}>
      {open ? <span>Check C11 is open: a new folder or a change to channels.yml is not served until the config-server restarts. </span> : null}
      {waiting > 0 ? <span>{`${waiting} ${plural(waiting, "service reports", "services report")} needing a config-server restart.`}</span> : null}
    </Banner>
  );
}
