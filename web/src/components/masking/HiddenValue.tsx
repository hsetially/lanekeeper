import { SafeText } from "@/components/app/SafeText";
import { Lock } from "@/icons";
import { cn } from "@/lib/utils";
import type { MarkerInfo } from "./marker";

// A value the server hid (S14b, M1), drawn as a token: an icon and the words "Hidden value", so it is never colour alone, and the
// length the marker states. A screen reader hears "Hidden value" and, when the mask names the setting, "for setting a.b.c". The
// token holds nothing that came from the value, and no title, label or data attribute: what it says is fixed text, the marker's
// length and the setting's name. (The design draws five dots; the dots say nothing to a screen reader and look like a value.)

type HiddenValueProps = {
  /** What the marker says about the value (its length). Null when the marker is not known. */
  info: MarkerInfo | null;
  /** The setting the mask names, for a screen reader. */
  setting?: string | null | undefined;
  className?: string;
};

export function HiddenValue({ info, setting = null, className }: HiddenValueProps) {
  return (
    <span
      data-slot="hidden-value"
      className={cn("inline-flex h-4 items-center gap-1 rounded-sm border border-dashed border-input bg-muted px-1 align-middle font-sans text-xs leading-none whitespace-nowrap text-muted-foreground", className)}
    >
      <Lock aria-hidden="true" className="size-3" />
      <span>Hidden value</span>
      {setting === null || setting === "" ? null : (
        <span className="sr-only">
          {" for setting "}
          <SafeText text={setting} />
        </span>
      )}
      {info?.chars == null ? null : (
        <>
          {" "}
          <span className="tabular-nums">{info.chars} chars</span>
        </>
      )}
    </span>
  );
}

/** A whole line that cannot be drawn because the hub's masks for it could not be checked: nothing of the line is shown. */
export function HiddenLine({ className }: { className?: string }) {
  return (
    <span data-slot="hidden-line" className={cn("inline-flex h-4 items-center gap-1 rounded-sm border border-dashed border-warning-border bg-warning-bg px-1 align-middle font-sans text-xs leading-none whitespace-nowrap text-warning-fg", className)}>
      <Lock aria-hidden="true" className="size-3" />
      <span>Hidden line</span>
      <span className="sr-only">: the hub&apos;s masking information for this line could not be checked, so none of it is shown</span>
    </span>
  );
}
