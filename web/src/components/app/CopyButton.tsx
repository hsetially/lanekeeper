import { useState, type RefObject } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Check, Copy, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";
import { writeClipboard } from "./clipboard";
import { COPIED, COPIED_HIDDEN, COULD_NOT_COPY, checkCopyable } from "./copy-text";
import { COPIED_MS, MESSAGE_MS, selectSource, useCopyFeedback, type Feedback } from "./copy-feedback";
import { Announce, StackedWord } from "./copy-parts";

// Copies a value that is not a secret: a request ID, a path, a hash, a setting path, a link. What a person sees:
//
//  - an icon button named for what it copies ("Copy request ID"), with the tooltip "Copy";
//  - on a click, a check and the word "Copied" in a small bubble for 1.5 s (no toast), and the same words said to a screen
//    reader through a visually hidden status;
//  - focus stays where it is and the button stays focusable and enabled; nothing moves (the icons share one box, the bubble
//    is in a portal, a labelled button reserves the width of both words);
//  - when the clipboard cannot be written, the bubble says "Couldn't copy. Select the text and copy it manually." and the text
//    in `source` is selected, so Control+C does what the button could not.
//
// What it will not do: copy a secret (`secret` is not a prop; a token is copied by secret/SecretCopyButton, from the one-time
// displays only), copy a line break or control character (checkCopyable, copy-text.ts), or keep the value anywhere.
// The check pops in with the app's own animation, which base.css switches off under prefers-reduced-motion.

type CopyButtonProps = {
  /** The exact text that goes on the clipboard: the full path, hash, setting path, link or request ID. */
  value: string;
  /** What is copied, for the accessible name: "path" gives "Copy path". Required: an icon never stands without a name. */
  what: string;
  /** Show the words as well as the icon. */
  showLabel?: boolean;
  /** The value is text with line breaks (a draft, the support details). Every other control character is still refused. */
  multiline?: boolean;
  /** The element that shows the text: it is selected when the copy fails. */
  source?: RefObject<HTMLElement | null>;
  className?: string;
  /** Never: a secret is not copied from here. */
  secret?: never;
};

export function CopyButton(props: CopyButtonProps) {
  // Written out for a caller that bypasses the types: a secret never reaches the generic button.
  if ((props as { secret?: unknown }).secret !== undefined) return null;
  return <Copier {...props} />;
}

function Copier({ value, what, showLabel = false, multiline = false, source, className }: Omit<CopyButtonProps, "secret">) {
  const { feedback, show, clear } = useCopyFeedback();
  const [hint, setHint] = useState(false);

  const copy = (): void => {
    const check = checkCopyable(value, { multiline });
    if (!check.ok) {
      show("problem", check.message, MESSAGE_MS);
      selectSource(source);
      return;
    }
    void writeClipboard(value).then((result) => {
      if (result === "ok") show("copied", check.hidden ? COPIED_HIDDEN : COPIED, check.hidden ? MESSAGE_MS : COPIED_MS);
      else {
        show("problem", COULD_NOT_COPY, MESSAGE_MS);
        selectSource(source);
      }
    });
  };

  const copied = feedback.kind === "copied";
  const open = feedback.kind !== "idle" || (hint && !showLabel);
  const name = `Copy ${what}`;
  return (
    <>
      <Tooltip
        open={open}
        onOpenChange={(next) => {
          // While a confirmation or a message is up it stays up for its time; only the hover and focus hint follows the pointer.
          setHint(next);
        }}
      >
        <TooltipTrigger asChild>
          {showLabel ? (
            <button
              type="button"
              data-slot="copy-button"
              data-copy={feedback.kind}
              aria-label={name}
              onClick={copy}
              className={cn("inline-flex h-6 shrink-0 cursor-pointer items-center justify-center rounded-sm border border-transparent bg-transparent px-2 text-sm font-medium text-foreground transition-colors duration-100 ease-out hover:bg-accent", className)}
            >
              <span className="grid">
                <StackedWord active={!copied} icon={<Copy aria-hidden="true" className="size-3.5" />} text={name} />
                <StackedWord active={copied} icon={<Check data-slot="copy-check" aria-hidden="true" className="size-3.5 animate-lk-pop-in text-success-fg" />} text={COPIED} />
              </span>
            </button>
          ) : (
            <button
              type="button"
              data-slot="copy-button"
              data-copy={feedback.kind}
              aria-label={name}
              onClick={copy}
              // A 24 px target (WCAG 2.5.8) that takes the 18 px of the design's button in the line (the 3 px margins), so a chip or a line of text does not grow for it.
              className={cn("group/copy -m-[3px] inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-sm text-muted-foreground transition-colors duration-100 ease-out hover:text-foreground focus-visible:text-foreground", className)}
            >
              <span className="pointer-events-none relative grid size-4 place-items-center rounded-sm transition-colors duration-100 ease-out group-hover/copy:bg-accent">
                <Copy aria-hidden="true" className={cn("col-start-1 row-start-1 size-3", copied && "invisible")} />
                {copied ? <Check data-slot="copy-check" aria-hidden="true" className="col-start-1 row-start-1 size-3 animate-lk-pop-in text-success-fg" /> : null}
              </span>
            </button>
          )}
        </TooltipTrigger>
        {/* The words are in the label, which Radix reads from its hidden copy of the tooltip; what is drawn is for the eyes. */}
        <TooltipContent aria-label={feedback.kind === "idle" ? "Copy" : feedback.message} onEscapeKeyDown={clear}>
          <Bubble feedback={feedback} />
        </TooltipContent>
      </Tooltip>
      <Announce feedback={feedback} />
    </>
  );
}

function Bubble({ feedback }: { feedback: Feedback }) {
  if (feedback.kind === "idle") return <span aria-hidden="true">Copy</span>;
  const Icon = feedback.kind === "copied" ? Check : TriangleAlert;
  return (
    <span aria-hidden="true" className="inline-flex items-start gap-1.5">
      <Icon className="mt-px size-3.5 shrink-0" />
      <span>{feedback.message}</span>
    </span>
  );
}
