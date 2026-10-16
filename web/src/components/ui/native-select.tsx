import type { ComponentProps } from "react";
import { ChevronDown } from "@/icons";
import { cn } from "@/lib/utils";

// A styled native <select> for plain lists. Radix Select is not used: it injects a <style> built with
// innerHTML, which Trusted Types and the CSP refuse (S12). Long or searchable lists use Popover plus Command.
export function NativeSelect({ className, children, ...props }: ComponentProps<"select">) {
  return (
    <div data-slot="native-select-wrapper" className="relative inline-flex w-full min-w-0">
      <select
        data-slot="native-select"
        className={cn(
          "h-7 w-full min-w-0 cursor-pointer appearance-none rounded-sm border border-input bg-background pr-7 pl-2 text-sm text-foreground transition-colors hover:border-muted-foreground focus-visible:border-ring focus-visible:shadow-[0_0_0_1px_var(--ring)] disabled:cursor-not-allowed disabled:bg-muted disabled:opacity-55 aria-invalid:border-destructive aria-invalid:shadow-[0_0_0_1px_var(--destructive)]",
          className,
        )}
        {...props}
      >
        {children}
      </select>
      <ChevronDown aria-hidden="true" className="pointer-events-none absolute top-1/2 right-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
    </div>
  );
}
