import { PickupState, type NfsPath, type SwimlaneDetail } from "@/api/generated/model";
import { NotifyServicesButton } from "./NotifyServicesButton";
import { RestartConfigServerButton } from "./RestartConfigServerButton";
import { RestartForFileButton } from "./RestartForFileButton";

// What the file page offers after a write, by the file's pickup state (D85). It is what the page's "Saved" banner holds in its
// action slot, next to the sentence "Services pick it up after a notify or a restart."
//  - waits for a notify or a restart: "Notify services" (this file) and "Restart services" (the services that read it);
//  - waits for the config-server to restart: "Restart config-server" (the hub offers it only while check C11 is open);
//  - anything else (live, live within its cache time, not stated): nothing, because there is nothing for a person to do.
// The buttons are the light part; each dialog is a chunk of its own, fetched when its button is first pressed.

export function FilePickupActions({ lane, path, pickup }: { lane: SwimlaneDetail; path: NfsPath; pickup: PickupState | null | undefined }) {
  if (pickup === PickupState.needs_config_server_restart) return <RestartConfigServerButton lane={lane} />;
  if (pickup === PickupState.needs_notify_or_restart) {
    return (
      <>
        <NotifyServicesButton lane={lane} path={path} />
        <RestartForFileButton lane={lane} path={path} />
      </>
    );
  }
  return null;
}
