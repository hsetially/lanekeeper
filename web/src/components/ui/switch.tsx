import type { ComponentProps } from "react";
import { Switch as SwitchPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

// 28x16 track, 12px thumb. Off is the control-boundary colour, on is the brand colour.
export function Switch({ className, ...props }: ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn("relative inline-flex h-4 w-7 shrink-0 cursor-pointer items-center rounded-full border-0 bg-input p-0 transition-colors data-[state=checked]:bg-primary disabled:cursor-not-allowed disabled:opacity-50", className)}
      {...props}
    >
      <SwitchPrimitive.Thumb data-slot="switch-thumb" className="pointer-events-none block size-3 translate-x-0.5 rounded-full bg-primary-foreground transition-transform duration-100 ease-out data-[state=checked]:translate-x-3.5" />
    </SwitchPrimitive.Root>
  );
}
