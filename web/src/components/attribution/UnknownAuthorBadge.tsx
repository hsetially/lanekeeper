import { badgeBase } from "@/components/ui/badge";
import { CircleHelp } from "@/icons";
import { cn } from "@/lib/utils";

// "Unknown author": nothing identified who changed the file (D73, source unknown). A dashed outline, italic words and a question
// mark, so it is told from a person by shape as well as by colour. It names no one and states no level.

export function UnknownAuthorBadge({ className }: { className?: string | undefined }) {
  return (
    <span data-slot="unknown-author" className={cn(badgeBase, "max-w-full border-dashed border-status-unknown-border bg-status-unknown-bg text-status-unknown-fg italic", className)}>
      <CircleHelp aria-hidden="true" />
      <span className="min-w-0 truncate">unknown author</span>
    </span>
  );
}
