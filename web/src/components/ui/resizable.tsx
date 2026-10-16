import type { ComponentProps } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import { cn } from "@/lib/utils";

export function ResizablePanelGroup({ className, ...props }: ComponentProps<typeof Group>) {
  return <Group data-slot="resizable-panel-group" className={cn("flex h-full w-full", className)} {...props} />;
}

export function ResizablePanel(props: ComponentProps<typeof Panel>) {
  return <Panel data-slot="resizable-panel" {...props} />;
}

type HandleProps = ComponentProps<typeof Separator> & { withHandle?: boolean };

// A 9px hit area around a 1px rule. The library sets role=separator, aria-valuenow and the arrow, Home and End keys.
// The handoff wants 16px steps (64px with Shift): the split pane in the file view tunes that where it is used.
export function ResizableHandle({ withHandle = false, className, ...props }: HandleProps) {
  return (
    <Separator
      data-slot="resizable-handle"
      className={cn(
        "relative flex w-px shrink-0 items-center justify-center bg-border transition-colors after:absolute after:inset-y-0 after:-left-1 after:w-[9px] after:content-[''] data-[separator=hover]:bg-input data-[separator=active]:bg-ring",
        className,
      )}
      {...props}
    >
      {withHandle ? <span aria-hidden="true" className="z-10 h-6 w-1 rounded-full bg-input" /> : null}
    </Separator>
  );
}
