import { memo } from "react";
import { SafeText } from "@/components/app/SafeText";
import { splitUnsafe } from "@/components/app/safe-text";

// SafeText for the cells of a grid or a table, where thousands of them are drawn every second of a scroll. A cell is already
// an element of its own, so its bidi isolation is set on that element (DataTable gives every cell `dir="auto"` and
// `unicode-bidi: isolate`, which is all a `<bdi>` is) and a text with nothing to mark is drawn as a bare text node: one
// element a cell fewer, in a view that makes tens of thousands of them. A text with a hidden or bidi character is drawn by
// SafeText, with its visible markers (S8, W21), exactly as everywhere else.

// The characters splitUnsafe marks: control, format, unpaired surrogates, separators, and the blank-looking letters.
const NEEDS_MARKS = /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}\u115F\u1160\u3164\uFFA0\u2800]|\u034F/u;

export const SafeCell = memo(function SafeCell({ text }: { text: string }) {
  // A tab is ordinary whitespace in a value (and is a control character, so it takes the longer way).
  if (!text.includes("\t") && !NEEDS_MARKS.test(text)) return <>{text}</>;
  return splitUnsafe(text).some((segment) => segment.kind === "mark") ? <SafeText text={text} /> : <>{text}</>;
});
