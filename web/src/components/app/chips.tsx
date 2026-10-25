import type { ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { CopyButton } from "./CopyButton";
import { SafeText } from "./SafeText";
import { truncateMiddle } from "./truncate";

// Path, hash, branch and setting chips: mono 12, 20px high, 4px radius, a copy button, and the full value in
// a tooltip when the display is shortened. Values are untrusted text, so they go through SafeText.
const chipClasses = "inline-flex h-5 max-w-full items-center gap-1 rounded-sm border border-border bg-background pr-0.5 pl-1.5 font-mono text-xs text-foreground";

type ChipProps = {
  /** The full value: what is copied and what assistive technology reads. */
  value: string;
  /** What is shown; defaults to the value. */
  display?: string;
  /** For the copy button's name: "path" gives "Copy path". */
  what: string;
  copy?: boolean;
  className?: string;
};

function Chip({ value, display = value, what, copy = true, className }: ChipProps) {
  const shortened = display !== value;
  const text: ReactNode = shortened ? (
    <>
      <span aria-hidden="true" className="font-mono">
        <SafeText text={display} />
      </span>
      <span className="sr-only">
        <SafeText text={value} />
      </span>
    </>
  ) : (
    <SafeText text={value} className="truncate font-mono" />
  );
  const body = shortened ? (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} className="inline-flex min-w-0 items-center rounded-sm">
          {text}
        </span>
      </TooltipTrigger>
      <TooltipContent className="font-mono">
        <SafeText text={value} />
      </TooltipContent>
    </Tooltip>
  ) : (
    text
  );
  return (
    <span data-slot="chip" className={cn(chipClasses, !copy && "pr-1.5", className)}>
      {body}
      {copy ? <CopyButton value={value} what={what} /> : null}
    </span>
  );
}

/** A file path, shortened in the middle. */
export function PathChip({ path, maxChars = 48, copy = true, className }: { path: string; maxChars?: number; copy?: boolean; className?: string }) {
  return <Chip value={path} display={truncateMiddle(path, maxChars)} what="path" copy={copy} {...(className ? { className } : {})} />;
}

/** A content hash, seven characters. */
export function HashChip({ hash, copy = true, className }: { hash: string; copy?: boolean; className?: string }) {
  return <Chip value={hash} display={Array.from(hash).slice(0, 7).join("")} what="hash" copy={copy} {...(className ? { className } : {})} />;
}

export function BranchChip({ branch, copy = true, className }: { branch: string; copy?: boolean; className?: string }) {
  return <Chip value={branch} what="branch" copy={copy} {...(className ? { className } : {})} />;
}

/** A setting path such as txInfinityOptions.remote-itm-teller.enableAccountSorting. */
export function SettingChip({ path, maxChars = 56, copy = true, className }: { path: string; maxChars?: number; copy?: boolean; className?: string }) {
  return <Chip value={path} display={truncateMiddle(path, maxChars)} what="setting path" copy={copy} {...(className ? { className } : {})} />;
}
