import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

/** A loading placeholder. Shape it like the final layout; mark the region aria-busy, not each block. */
export function Skeleton({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="skeleton" aria-hidden="true" className={cn("animate-lk-pulse rounded-sm bg-muted", className)} {...props} />;
}
