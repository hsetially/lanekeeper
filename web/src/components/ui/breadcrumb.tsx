import type { ComponentProps } from "react";
import { Slot } from "radix-ui";
import { ChevronRight } from "@/icons";
import { cn } from "@/lib/utils";

export function Breadcrumb(props: ComponentProps<"nav">) {
  return <nav data-slot="breadcrumb" aria-label="Breadcrumb" {...props} />;
}

export function BreadcrumbList({ className, ...props }: ComponentProps<"ol">) {
  return <ol data-slot="breadcrumb-list" className={cn("flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground", className)} {...props} />;
}

export function BreadcrumbItem({ className, ...props }: ComponentProps<"li">) {
  return <li data-slot="breadcrumb-item" className={cn("inline-flex min-w-0 items-center gap-1.5", className)} {...props} />;
}

export function BreadcrumbLink({ asChild, className, ...props }: ComponentProps<"a"> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "a";
  return <Comp data-slot="breadcrumb-link" className={cn("truncate rounded-sm text-muted-foreground transition-colors hover:text-foreground", className)} {...props} />;
}

export function BreadcrumbPage({ className, ...props }: ComponentProps<"span">) {
  return <span data-slot="breadcrumb-page" aria-current="page" className={cn("truncate font-medium text-foreground", className)} {...props} />;
}

export function BreadcrumbSeparator({ children, className, ...props }: ComponentProps<"li">) {
  return (
    <li data-slot="breadcrumb-separator" role="presentation" aria-hidden="true" className={cn("[&>svg]:size-3.5", className)} {...props}>
      {children ?? <ChevronRight />}
    </li>
  );
}
