import { cn } from "@/lib/utils";
import { CopyButton } from "./CopyButton";
import { SafeText } from "./SafeText";

/**
 * "Request ID" with the value in mono and a copy button. Every error shows one, so a report can name the request. An error that
 * came with a response can also offer its details for support (support-details.ts): a second, quieter button after the id.
 */
export function RequestId({ id, details, className }: { id: string; /** The text of "Copy details for support": the request, problem, type, status and time. */ details?: string; className?: string }) {
  return (
    <span data-slot="request-id" className={cn("inline-flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground", className)}>
      <span>Request ID</span>
      <SafeText text={id} className="font-mono text-foreground" />
      <CopyButton value={id} what="request ID" />
      {details === undefined ? null : <CopyButton value={details} what="details for support" showLabel multiline className="h-5 px-1.5 text-xs font-normal text-muted-foreground hover:text-foreground" />}
    </span>
  );
}
