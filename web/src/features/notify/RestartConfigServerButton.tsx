import { lazy, Suspense, useRef, useState } from "react";
import type { SwimlaneDetail } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { Server } from "@/icons";
import { useServiceWriteGate } from "./gates";

// The button that starts a config-server restart: on the Services tab (in the banner that says one is needed) and on the file
// page for a file whose pickup state is "Needs config-server restart". It is on screen for everyone and disabled, with the reason
// in a tooltip, for a person below Operator, while live updates are paused and while the agent is offline. It reads nothing until it
// is pressed: the offer is the dialog's. The dialog is its own chunk, fetched when the button is first pressed (P10).
//
// Its accessible name is longer than its words. A swimlane can have a service called config-server, whose row on the Services tab
// has a button named "Restart config-server" (it restarts that one service, no typing). Two buttons of one name that do different
// things cannot be told apart in a screen reader's list of buttons, so this one says what is different about it: the dialog asks for
// the Deployment's name to be typed. The visible words are the start of the name (WCAG 2.5.3, Label in Name).

const LABEL = "Restart config-server";

const RestartConfigServerDialog = lazy(() => import("./RestartConfigServerDialog").then((module) => ({ default: module.RestartConfigServerDialog })));

export function RestartConfigServerButton({ lane, variant = "secondary" }: { lane: SwimlaneDetail; variant?: "secondary" | "default" }) {
  const gate = useServiceWriteGate(lane.agent);
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  return (
    <>
      <GatedButton
        ref={trigger}
        gate={gate}
        variant={variant}
        size="sm"
        aria-label={`${LABEL} (type its name to confirm)`}
        onClick={() => {
          setOpen(true);
        }}
      >
        <Server data-icon="inline-start" />
        {LABEL}
      </GatedButton>
      {open ? (
        <Suspense fallback={null}>
          <RestartConfigServerDialog
            lane={lane}
            open
            returnFocusRef={trigger}
            onClose={() => {
              setOpen(false);
            }}
          />
        </Suspense>
      ) : null}
    </>
  );
}
