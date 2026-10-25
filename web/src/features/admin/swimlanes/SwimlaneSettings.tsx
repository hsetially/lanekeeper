import { useEffect, useRef } from "react";
import { SafeText } from "@/components/app/SafeText";
import { AgentAccount, type AgentKnowledge } from "./AgentAccount";
import { AutoNotifySwitch } from "./AutoNotifySwitch";
import { JoinTokenCard } from "./JoinTokenCard";
import { TenantsEditor } from "./TenantsEditor";

// Everything an Admin sets for ONE swimlane, in one place: its tenants, its auto-notify switch, the service account its agent runs as
// and the fallback join token. It is for the swimlane chosen in the list, so a page of forty swimlanes asks the hub for the settings
// of one, not of forty. Moving to another swimlane starts these parts afresh (the card is keyed by swimlane in the section), which also
// forgets a join token that was on screen.

export function SwimlaneSettings({ swimlane, known }: { swimlane: string; known: AgentKnowledge }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    // The person chose this swimlane with Manage, so it is where they look next.
    heading.current?.focus();
    heading.current?.scrollIntoView({ block: "nearest" });
  }, []);
  return (
    <section aria-label={`Settings for ${swimlane}`} className="flex flex-col gap-5 rounded-md border border-border bg-card p-4">
      <h3 ref={heading} tabIndex={-1} className="text-sm font-semibold outline-none">
        Settings for <SafeText text={swimlane} className="font-mono" />
      </h3>
      <TenantsEditor swimlane={swimlane} />
      <AutoNotifySwitch swimlane={swimlane} />
      <AgentAccount swimlane={swimlane} known={known} />
      <JoinTokenCard swimlane={swimlane} />
    </section>
  );
}
