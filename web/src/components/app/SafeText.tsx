import { cn } from "@/lib/utils";
import { splitUnsafe } from "./safe-text";

/**
 * Renders untrusted text with bidi controls, zero-width and other invisible characters shown as visible markers.
 * Use it for every path, name, setting value, diff line and search hit. The <bdi> isolates the text, so a
 * right-to-left name cannot reorder what is around it.
 */
export function SafeText({ text, className }: { text: string; className?: string }) {
  const segments = splitUnsafe(text);
  return (
    <bdi data-slot="safe-text" className={className}>
      {segments.map((segment, i) =>
        segment.kind === "text" ? (
          segment.text
        ) : (
          <span
            // The position is the identity: a marker has no state and the list changes only with the text.
            key={i}
            role="img"
            aria-label={`${segment.codePoint} ${segment.name}`}
            title={`${segment.codePoint} ${segment.name}`}
            className={cn("mx-px rounded-sm border border-warning-border bg-warning-bg px-0.5 font-mono text-warning-fg")}
          >
            {segment.text}
          </span>
        ),
      )}
    </bdi>
  );
}
