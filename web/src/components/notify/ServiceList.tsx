import { useId } from "react";
import type { ServiceRef } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";

// The services a confirmation names: the ones a notification is for, or the ones a restart would take away. Names come from the
// cluster report, so they go through SafeText. A note is the screen's own words for one service (for example the setting the agent
// reported), never something the server wrote. `label` is what the list is, with its count ("3 services affected"): it is shown
// above the list and it names the region.

export type ListedService = { readonly service: ServiceRef; readonly note?: string };

export function ServiceList({ label, services }: { label: string; services: readonly ListedService[] }) {
  const id = useId();
  if (services.length === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      <p id={id} className="text-xs font-medium text-muted-foreground">
        {label}
      </p>
      <div role="region" aria-labelledby={id} {...FOCUSABLE_SCROLLER} className="max-h-32 overflow-auto rounded-md border border-border bg-muted px-2 py-1.5">
        <ul className="flex flex-col gap-0.5">
          {services.map(({ service, note }) => (
            <li key={`${service.namespace}/${service.name}`} className="text-xs break-all">
              <SafeText text={`${service.namespace}/${service.name}`} className="font-mono" />
              {note === undefined ? null : (
                <>
                  {" "}
                  <span className="text-muted-foreground">{note}</span>
                </>
              )}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
