import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// tailwind-merge has to know the handoff's own utility names, or it files `text-mono` under text colours
// and `bg-diff-empty` under background colours and drops one of a pair that should both stay.
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      text: ["mono"],
      tracking: ["caps"],
      shadow: ["popover", "pressed"],
      animate: ["lk-spin", "lk-pulse", "lk-fade-in", "lk-pop-in"],
    },
    classGroups: {
      "bg-image": [{ bg: ["diff-empty"] }],
    },
  },
});

/** Join class names; later utilities win within one property group. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
