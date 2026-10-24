import type { ComponentProps, KeyboardEvent, RefObject } from "react";
import { Group, Panel, Separator, type PanelImperativeHandle } from "react-resizable-panels";
import { cn } from "@/lib/utils";

export function ResizablePanelGroup({ className, ...props }: ComponentProps<typeof Group>) {
  return <Group data-slot="resizable-panel-group" className={cn("flex h-full w-full", className)} {...props} />;
}

export function ResizablePanel(props: ComponentProps<typeof Panel>) {
  return <Panel data-slot="resizable-panel" {...props} />;
}

/** The handoff's keys for a split pane handle: Left and Right move it 16 px, 64 px with Shift. */
const STEP = 16;
const BIG_STEP = 64;

type HandleProps = ComponentProps<typeof Separator> & {
  withHandle?: boolean;
  /**
   * The panel before the handle (the one on its left). Given, the arrow keys move the handle by pixels, as the keymap says:
   * the library steps by a share of the group, which is a different distance on every window. Home and End are the library's:
   * the panel before the handle goes to its least, and to nothing if it is `collapsible` ("Home/End collapse" in the keymap).
   */
  panel?: RefObject<PanelImperativeHandle | null>;
};

// A 9px hit area around a 1px rule. The library sets role=separator, aria-valuenow and the arrow, Home and End keys.
export function ResizableHandle({ withHandle = false, panel, className, onKeyDownCapture, ...props }: HandleProps) {
  // The library listens for the key on the separator itself and leaves a key that was already handled alone, so a handler in the
  // capture phase (which runs before it) that takes the arrow keys is the one that moves the handle.
  const step = (event: KeyboardEvent<HTMLDivElement>) => {
    onKeyDownCapture?.(event);
    const target = panel?.current;
    if (!target || event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    // A panel that Home collapsed has no width to step from, and a step of 16 px from nothing is inside the range where the library
    // shuts it again: Right brings it back as it was, Left has nothing to take away.
    if (target.isCollapsed()) {
      if (event.key === "ArrowRight") target.expand();
      return;
    }
    const distance = event.shiftKey ? BIG_STEP : STEP;
    target.resize(target.getSize().inPixels + (event.key === "ArrowRight" ? distance : -distance));
  };
  return (
    <Separator
      data-slot="resizable-handle"
      className={cn(
        "relative flex w-px shrink-0 items-center justify-center bg-border transition-colors after:absolute after:inset-y-0 after:-left-1 after:w-[9px] after:content-[''] data-[separator=hover]:bg-input data-[separator=active]:bg-ring",
        className,
      )}
      onKeyDownCapture={step}
      {...props}
    >
      {withHandle ? <span aria-hidden="true" className="z-10 h-6 w-1 rounded-full bg-input" /> : null}
    </Separator>
  );
}
