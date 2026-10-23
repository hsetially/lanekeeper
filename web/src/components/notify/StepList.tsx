import { useId } from "react";
import { SafeText } from "@/components/app/SafeText";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";
import { Check, CircleX, Hourglass, Minus } from "@/icons";
import type { Step, StepState } from "./steps";

// One line for each restart request of a "restart all": the service and what the hub answered to its request. Each answer has its
// words and its icon, so it is never a colour alone. The words say what the hub did with the REQUEST ("accepted"); nothing here says
// that a service restarted, because the contract has no signal for it.

const WORDS: Record<StepState, string> = {
  waiting: "waiting",
  sending: "sending",
  accepted: "accepted",
  "sent-for-approval": "sent for approval",
  failed: "not accepted",
  "not-attempted": "not attempted",
};

function Mark({ state }: { state: StepState }) {
  switch (state) {
    case "accepted":
      return <Check aria-hidden="true" className="text-success-fg" />;
    case "sent-for-approval":
      return <Hourglass aria-hidden="true" className="text-status-approval-fg" />;
    case "failed":
      return <CircleX aria-hidden="true" className="text-danger-fg" />;
    case "not-attempted":
      return <Minus aria-hidden="true" className="text-muted-foreground" />;
    default:
      return <span aria-hidden="true" className="inline-block size-3" />;
  }
}

export function StepList({ label, steps }: { label: string; steps: readonly Step[] }) {
  const id = useId();
  if (steps.length === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      <p id={id} className="text-xs font-medium text-muted-foreground">
        {label}
      </p>
      <div role="region" aria-labelledby={id} {...FOCUSABLE_SCROLLER} className="max-h-40 overflow-auto rounded-md border border-border bg-muted px-2 py-1.5">
        <ul className="flex flex-col gap-0.5">
          {steps.map((step) => (
            <li key={`${step.service.namespace}/${step.service.name}`} className="flex items-center gap-1.5 text-xs [&_svg]:size-3 [&_svg]:shrink-0">
              <Mark state={step.state} />
              <span className="min-w-0 break-all">
                <SafeText text={`${step.service.namespace}/${step.service.name}`} className="font-mono" />{" "}
                <span className="text-muted-foreground">{WORDS[step.state]}{step.request === undefined ? "" : `, request AP-${step.request}`}</span>
              </span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
