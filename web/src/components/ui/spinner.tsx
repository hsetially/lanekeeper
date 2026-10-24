import type { ComponentProps } from "react";
import { LoaderCircle } from "@/icons";
import { cn } from "@/lib/utils";

/** The loading spinner (Lucide loader-circle). Decorative: the busy state is announced by aria-busy on the region. */
export function Spinner({ className, ...props }: ComponentProps<typeof LoaderCircle>) {
  return <LoaderCircle data-slot="spinner" aria-hidden="true" className={cn("size-3.5 shrink-0 animate-lk-spin", className)} {...props} />;
}
