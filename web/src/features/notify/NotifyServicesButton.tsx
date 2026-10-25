import { lazy, Suspense, useRef, useState } from "react";
import type { NfsPath, SwimlaneDetail } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { Bell } from "@/icons";
import { useServiceWriteGate } from "./gates";
import { planForFile, planForServices, type NotifyPlan } from "./paths";

// Where a notification starts. On the Services tab it announces the files that services are waiting on (`path` left out); on the
// file page, after a write, it announces that one file (`path`). The button is on screen for everyone and is disabled, with the
// reason in a tooltip, for a person below Operator, while live updates are paused, while the agent is offline, and (on the tab) when
// no service is waiting on a file. What the person reads in the dialog is what is sent: the plan is fixed when the dialog opens.
// The dialog is its own chunk, fetched when the button is first pressed: the file page's first load (P10) carries the button and not
// the code of a dialog.

const NotifyServicesDialog = lazy(() => import("./NotifyServicesDialog").then((module) => ({ default: module.NotifyServicesDialog })));

type Props = {
  lane: SwimlaneDetail;
  /** The file that was just written. Without it, the files that services are waiting on. */
  path?: NfsPath;
  variant?: "secondary" | "default";
};

export function NotifyServicesButton({ lane, path, variant = "secondary" }: Props) {
  const base = useServiceWriteGate(lane.agent);
  const [opened, setOpened] = useState<NotifyPlan | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const plan = path === undefined ? planForServices(lane.services) : planForFile(path, lane.services);
  const gate = base.allowed && plan.paths.length === 0 ? { allowed: false, reason: "No service is waiting on a file." } : base;

  return (
    <>
      <GatedButton
        ref={trigger}
        gate={gate}
        variant={variant}
        size="sm"
        onClick={() => {
          setOpened(plan);
        }}
      >
        <Bell data-icon="inline-start" />
        Notify services
      </GatedButton>
      {opened === null ? null : (
        <Suspense fallback={null}>
          <NotifyServicesDialog
            lane={lane}
            plan={opened}
            open
            returnFocusRef={trigger}
            onClose={() => {
              setOpened(null);
            }}
          />
        </Suspense>
      )}
    </>
  );
}
