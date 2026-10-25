import { useId, type ComponentProps } from "react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useWriteGate, type GateResult } from "./write-gate";

type ButtonProps = ComponentProps<typeof Button>;

type GatedButtonProps = ButtonProps & {
  /** A precomputed result, for screens with their own reasons. Otherwise minRole and the session decide. */
  gate?: GateResult;
  minRole?: string;
  needsConnection?: boolean;
};

/**
 * A button that is disabled with a reason instead of hidden. It stays focusable (aria-disabled, not disabled), ignores
 * clicks, shows the reason in a tooltip and exposes it as the accessible description. The reason names the role
 * needed and who can grant it. The server still enforces the permission.
 */
export function GatedButton({ gate, minRole, needsConnection, onClick, children, ...props }: GatedButtonProps) {
  const fromSession = useWriteGate(minRole, needsConnection === undefined ? {} : { needsConnection });
  const { allowed, reason } = gate ?? fromSession;
  const reasonId = useId();

  if (allowed) {
    return (
      <Button onClick={onClick} {...props}>
        {children}
      </Button>
    );
  }
  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button {...props} aria-disabled="true" aria-describedby={reasonId} data-gated="" onClick={(event) => event.preventDefault()}>
            {children}
          </Button>
        </TooltipTrigger>
        <TooltipContent>{reason}</TooltipContent>
      </Tooltip>
      <span id={reasonId} className="sr-only">
        {reason}
      </span>
    </>
  );
}
