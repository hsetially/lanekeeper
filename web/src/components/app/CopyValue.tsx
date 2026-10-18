import { useRef, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { CopyButton } from "./CopyButton";
import { SafeText } from "./SafeText";

// "value [copy]": a value with its copy button, for a drawer, a detail line or a header, where the button must not clutter. The
// button is invisible until the pointer is over the value or focus is inside it, and then it is the same button as everywhere
// else. It is hidden by opacity alone, so it is always in the keyboard order, always found by a screen reader, and appears when
// Tab reaches it; where there is no hover (touch) it is always shown, and it stays while it says "Copied" or why it could not.
//
// The value is untrusted text (S8), so it is drawn through SafeText unless the caller gives its own children (a short form of a
// long value). What is copied is `value`, exactly, whatever is drawn. Do not put one in a dense table row (a DataTable or tree
// row has the keyboard cursor, and a tab stop in every row would make the table longer to cross): put it in the row's drawer.

/** The group name and the classes of the reveal, for a layout that cannot use CopyValue as it is (a heading, whose name must not take the button's). */
export const COPY_GROUP = "group/copy-value";
export const COPY_REVEAL =
  "opacity-0 group-hover/copy-value:opacity-100 group-focus-within/copy-value:opacity-100 focus-visible:opacity-100 data-[copy=copied]:opacity-100 data-[copy=problem]:opacity-100 [@media(hover:none)]:opacity-100";

type CopyValueProps = {
  /** The exact text to copy. */
  value: string;
  /** What it is, for the button's name: "path" gives "Copy path". */
  what: string;
  /** What is drawn, when it is not the value itself. */
  children?: ReactNode;
  className?: string;
  /** Classes for the value's own box (font, truncation). */
  valueClassName?: string;
};

export function CopyValue({ value, what, children, className, valueClassName }: CopyValueProps) {
  const source = useRef<HTMLSpanElement>(null);
  return (
    <span data-slot="copy-value" className={cn(COPY_GROUP, "inline-flex max-w-full min-w-0 items-center gap-0.5", className)}>
      <span ref={source} className={cn("min-w-0", valueClassName)}>
        {children ?? <SafeText text={value} />}
      </span>
      <CopyButton value={value} what={what} source={source} className={COPY_REVEAL} />
    </span>
  );
}
