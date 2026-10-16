import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export function Textarea({ className, ...props }: ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "min-h-16 w-full min-w-0 resize-y rounded-sm border border-input bg-background px-2 py-1.5 text-sm text-foreground transition-colors placeholder:text-muted-foreground hover:border-muted-foreground focus-visible:border-ring focus-visible:shadow-[0_0_0_1px_var(--ring)] disabled:cursor-not-allowed disabled:bg-muted disabled:opacity-55 aria-invalid:border-destructive aria-invalid:shadow-[0_0_0_1px_var(--destructive)]",
        className,
      )}
      {...props}
    />
  );
}
