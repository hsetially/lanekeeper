import { lazy, Suspense, useRef, useState } from "react";
import type { NfsPath, SwimlaneDetail } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { RotateCw } from "@/icons";
import { useServiceWriteGate } from "./gates";

// "Restart services" on the file page, after a write: restarts the services that read this file (the hub lists them when the dialog
// opens). On screen for everyone, disabled with the reason in a tooltip for a person below Operator, while live updates are paused
// and while the agent is offline. It reads nothing until it is pressed. The dialog is its own chunk, fetched when the button is first
// pressed (P10).

const RestartForFileDialog = lazy(() => import("./RestartForFileDialog").then((module) => ({ default: module.RestartForFileDialog })));

export function RestartForFileButton({ lane, path, variant = "secondary" }: { lane: SwimlaneDetail; path: NfsPath; variant?: "secondary" | "default" }) {
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
        onClick={() => {
          setOpen(true);
        }}
      >
        <RotateCw data-icon="inline-start" />
        Restart services
      </GatedButton>
      {open ? (
        <Suspense fallback={null}>
          <RestartForFileDialog
            lane={lane}
            path={path}
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
