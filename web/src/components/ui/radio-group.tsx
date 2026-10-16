import type { ComponentProps } from "react";
import { RadioGroup as RadioGroupPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

export function RadioGroup({ className, ...props }: ComponentProps<typeof RadioGroupPrimitive.Root>) {
  return <RadioGroupPrimitive.Root data-slot="radio-group" className={cn("grid gap-2", className)} {...props} />;
}

export function RadioGroupItem({ className, ...props }: ComponentProps<typeof RadioGroupPrimitive.Item>) {
  return (
    <RadioGroupPrimitive.Item
      data-slot="radio-group-item"
      className={cn("relative inline-flex size-3.5 shrink-0 cursor-pointer items-center justify-center rounded-full border border-input bg-background p-0 after:absolute after:-inset-[5px] after:content-[''] disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:border-primary", className)}
      {...props}
    >
      <RadioGroupPrimitive.Indicator className="block size-1.5 rounded-full bg-primary" />
    </RadioGroupPrimitive.Item>
  );
}
