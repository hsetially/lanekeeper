import type { ReactNode } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { X } from "@/icons";
import { cn } from "@/lib/utils";

type BannerProps = {
  variant: "info" | "success" | "warning" | "danger";
  title: string;
  /** Buttons or links on the right. */
  action?: ReactNode;
  /** Replaces the icon of the variant (the paused banner's wifi-off). Decorative: the title says it. */
  icon?: ReactNode;
  /** Only a success banner can be dismissed: a warning or an error stays until the cause is gone. */
  onDismiss?: () => void;
  children?: ReactNode;
  className?: string;
};

/** A full-width Alert: NFS ahead, PR open, Pending approval, Conflict, Saved (restart needed), Offline, Locked. */
export function Banner({ variant, title, action, icon, onDismiss, children, className }: BannerProps) {
  return (
    <Alert variant={variant} {...(icon === undefined ? {} : { icon })} className={cn("items-center rounded-none border-x-0 px-4", className)}>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
        <AlertTitle>{title}</AlertTitle>
        {children ? <AlertDescription className="min-w-0 flex-1">{children}</AlertDescription> : null}
      </div>
      {action ? <div className="flex shrink-0 items-center gap-2">{action}</div> : null}
      {variant === "success" && onDismiss ? (
        <Button variant="ghost" size="icon-sm" aria-label="Dismiss" onClick={onDismiss} className="text-success-fg">
          <X />
        </Button>
      ) : null}
    </Alert>
  );
}
