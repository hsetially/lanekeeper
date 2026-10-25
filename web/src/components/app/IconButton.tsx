import type { ComponentProps, ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

type IconButtonProps = Omit<ComponentProps<typeof Button>, "children" | "size" | "aria-label" | "aria-pressed"> & {
  /** Required: the accessible name and the tooltip. An icon never stands without a label or a tooltip. */
  label: string;
  /** Key hint shown in the tooltip: "/", "⌘K". */
  shortcut?: string;
  size?: "icon" | "icon-sm";
  /** For a toggle (Match case, pin): sets aria-pressed. */
  pressed?: boolean;
  /** The icon. */
  children: ReactNode;
};

/** A button that is only an icon (28px, or 24px small), with its name in a tooltip and the shortcut hint. */
export function IconButton({ label, shortcut, size = "icon", pressed, variant = "ghost", children, ...props }: IconButtonProps) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant={variant} size={size} aria-label={label} {...(pressed === undefined ? {} : { "aria-pressed": pressed })} {...props}>
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {label}
        {shortcut ? <Kbd className="border-background/30 bg-transparent text-background">{shortcut}</Kbd> : null}
      </TooltipContent>
    </Tooltip>
  );
}
