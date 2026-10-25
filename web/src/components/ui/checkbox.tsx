import type { ComponentProps } from "react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";
import { Check, Minus } from "@/icons";
import { cn } from "@/lib/utils";

// 14px box, 3px radius. checked can be "indeterminate" (select all), which Radix exposes as aria-checked=mixed.
// The ::after widens the pointer target around the small box without changing the layout.
export function Checkbox({ className, ...props }: ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "group relative inline-flex size-3.5 shrink-0 cursor-pointer items-center justify-center rounded-[3px] border border-input bg-background p-0 text-primary-foreground transition-colors after:absolute after:-inset-[5px] after:content-[''] disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:border-primary data-[state=checked]:bg-primary data-[state=indeterminate]:border-primary data-[state=indeterminate]:bg-primary",
        className,
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator data-slot="checkbox-indicator" className="flex items-center justify-center">
        <Check className="hidden size-3 group-data-[state=checked]:block" aria-hidden="true" />
        <Minus className="hidden size-3 group-data-[state=indeterminate]:block" aria-hidden="true" />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  );
}
