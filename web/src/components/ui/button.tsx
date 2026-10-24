import type { MouseEvent } from "react";
import type { ComponentProps } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";
import { cn } from "@/lib/utils";
import { Spinner } from "./spinner";

// Handoff Button: default (primary), secondary, ghost, destructive; 28px (24px small); 4px radius; 13/500.
// Active is the hover colour plus an inset shadow and a 1px drop. Disabled is 50% with a not-allowed cursor.
export const buttonVariants = cva(
  "inline-flex shrink-0 cursor-pointer select-none items-center justify-center gap-1.5 whitespace-nowrap rounded-sm border text-sm font-medium transition-colors duration-100 ease-out active:translate-y-px active:shadow-pressed disabled:cursor-not-allowed disabled:opacity-50 aria-disabled:cursor-not-allowed aria-disabled:opacity-50 aria-busy:cursor-progress [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-3.5",
  {
    variants: {
      variant: {
        default: "border-primary bg-primary text-primary-foreground hover:bg-primary-hover active:bg-primary-hover",
        secondary: "border-border bg-secondary text-secondary-foreground hover:bg-secondary-hover active:bg-secondary-hover",
        ghost: "border-transparent bg-transparent text-foreground hover:bg-accent active:bg-accent",
        destructive: "border-destructive bg-destructive text-destructive-foreground hover:bg-destructive-hover active:bg-destructive-hover",
      },
      size: {
        default: "h-7 px-2.5",
        sm: "h-6 px-2",
        icon: "size-7",
        "icon-sm": "size-6",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

type ButtonProps = ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
    /** Spinner, aria-busy, clicks ignored. Keeps focus (it is not natively disabled). Change the label to a verb: "Working…". */
    loading?: boolean;
  };

export function Button({ className, variant = "default", size = "default", asChild = false, loading = false, onClick, children, ...props }: ButtonProps) {
  const Comp = asChild ? Slot.Root : "button";
  const handleClick = (event: MouseEvent<HTMLButtonElement>) => {
    if (loading) {
      event.preventDefault();
      return;
    }
    onClick?.(event);
  };
  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      {...(asChild ? {} : { type: "button" as const })}
      {...(loading ? { "aria-busy": true, "aria-disabled": true } : {})}
      className={cn(buttonVariants({ variant, size }), className)}
      onClick={handleClick}
      {...props}
    >
      {asChild ? (
        children
      ) : (
        <>
          {loading ? <Spinner data-icon="inline-start" /> : null}
          {children}
        </>
      )}
    </Comp>
  );
}
