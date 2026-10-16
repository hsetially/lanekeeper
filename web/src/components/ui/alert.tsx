import type { ComponentProps, ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { CircleCheck, CircleX, Info, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";

const alertVariants = cva("flex w-full items-start gap-2 rounded-md border px-3 py-2 text-sm [&_svg]:mt-px [&_svg]:size-4 [&_svg]:shrink-0", {
  variants: {
    variant: {
      info: "border-info-border bg-info-bg text-info-fg",
      success: "border-success-border bg-success-bg text-success-fg",
      warning: "border-warning-border bg-warning-bg text-warning-fg",
      danger: "border-danger-border bg-danger-bg text-danger-fg",
    },
  },
  defaultVariants: { variant: "info" },
});

const ICONS = { info: Info, success: CircleCheck, warning: TriangleAlert, danger: CircleX } as const;

type AlertProps = Omit<ComponentProps<"div">, "role"> & VariantProps<typeof alertVariants> & { icon?: ReactNode };

/** Callout. A status for info, success and warning; an alert (assertive) for danger. Always carries an icon. */
export function Alert({ className, variant = "info", icon, children, ...props }: AlertProps) {
  const Icon = ICONS[variant ?? "info"];
  return (
    <div data-slot="alert" data-variant={variant} role={variant === "danger" ? "alert" : "status"} className={cn(alertVariants({ variant }), className)} {...props}>
      {icon ?? <Icon aria-hidden="true" />}
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">{children}</div>
    </div>
  );
}

export function AlertTitle({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="alert-title" className={cn("font-medium", className)} {...props} />;
}

export function AlertDescription({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="alert-description" className={cn("text-sm", className)} {...props} />;
}
