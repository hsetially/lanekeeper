import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Check, Copy } from "@/icons";
import { cn } from "@/lib/utils";

type Status = "idle" | "copied" | "failed";

const COPIED_MS = 1500;
const FAILED_MS = 3000;

type CopyButtonProps = {
  /** The exact text that goes on the clipboard: the full path, hash, setting path or request ID. */
  value: string;
  /** What is copied, for the accessible name: "path" gives "Copy path". */
  what: string;
  /** Show the word as well as the icon. */
  showLabel?: boolean;
  className?: string;
};

/**
 * Copies a value. The icon becomes a check and the name "Copied" for 1.5 s, announced politely; a failure says so.
 * Never use it for a secret (a revealed value, a token): those are never copied from the page.
 */
export function CopyButton({ value, what, showLabel = false, className }: CopyButtonProps) {
  const [status, setStatus] = useState<Status>("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const settle = (next: Status, ms: number) => {
    setStatus(next);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setStatus("idle"), ms);
  };

  const copy = () => {
    // The clipboard can be absent (insecure context) or refuse (permissions); neither may throw into the page.
    const clipboard = navigator.clipboard as Clipboard | undefined;
    if (!clipboard) {
      settle("failed", FAILED_MS);
      return;
    }
    clipboard.writeText(value).then(
      () => {
        settle("copied", COPIED_MS);
      },
      () => {
        settle("failed", FAILED_MS);
      },
    );
  };

  const label = status === "copied" ? "Copied" : status === "failed" ? "Copy failed" : `Copy ${what}`;
  return (
    <>
      <Button
        variant="ghost"
        size={showLabel ? "sm" : "icon-sm"}
        aria-label={showLabel ? undefined : label}
        title={label}
        onClick={copy}
        className={cn(!showLabel && "size-4 shrink-0 text-muted-foreground hover:text-foreground", className)}
      >
        {status === "copied" ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
        {showLabel ? label : null}
      </Button>
      <span aria-live="polite" className="sr-only">
        {status === "copied" ? "Copied" : status === "failed" ? "Copy failed" : ""}
      </span>
    </>
  );
}
