import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Inbox } from "@/icons";
import { cn } from "@/lib/utils";

/**
 * The design's empty state (States index, Empty): an icon, what is missing, why, and the one next step the role can take.
 * Every screen says its own words; this is only the shape, so that all of them look and read the same.
 */
export function EmptyState({
  icon: Icon = Inbox,
  title,
  children,
  action,
  as: Heading = "p",
  className,
}: {
  icon?: LucideIcon;
  title: string;
  /** What is missing and why, one or two sentences. */
  children?: ReactNode;
  /** The next step, if the role can take it: a button or a link. When it cannot, say who can in the text instead. */
  action?: ReactNode;
  /** The title is a heading only when it is the screen's only title (a page with nothing to show). */
  as?: "p" | "h1" | "h2";
  className?: string;
}) {
  return (
    <div data-slot="empty-state" className={cn("flex flex-col items-center justify-center gap-2 px-6 py-10 text-center", className)}>
      <Icon aria-hidden="true" className="size-5 text-muted-foreground" />
      <Heading className="text-base font-semibold">{title}</Heading>
      {children ? <p className="max-w-sm text-sm text-muted-foreground">{children}</p> : null}
      {action ? <div className="mt-1 flex flex-wrap items-center justify-center gap-2">{action}</div> : null}
    </div>
  );
}
