import { cn } from "@/lib/utils";
import { CopyButton } from "./CopyButton";
import { SafeText } from "./SafeText";

/** "Request ID" with the value in mono and a copy button. Every error shows one, so a report can name the request. */
export function RequestId({ id, className }: { id: string; className?: string }) {
  return (
    <span data-slot="request-id" className={cn("inline-flex items-center gap-1.5 text-xs text-muted-foreground", className)}>
      <span>Request ID</span>
      <SafeText text={id} className="font-mono text-foreground" />
      <CopyButton value={id} what="request ID" />
    </span>
  );
}
