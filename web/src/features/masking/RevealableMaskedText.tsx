import type { ReactNode } from "react";
import type { Mask } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { GatedButton } from "@/components/app/GatedButton";
import { useWriteGate } from "@/components/app/write-gate";
import { MaskedTextView } from "@/components/masking/MaskedTextView";
import type { PlannedMark } from "@/components/masking/plan";
import { Button } from "@/components/ui/button";
import { Eye } from "@/icons";
import { cn } from "@/lib/utils";
import { RevealDialog } from "./RevealDialog";
import { MAX_SHOWN_CHARS, useReveal, type RevealState } from "./use-reveal";

// A text the server masked, with the Reveal control of the File page (the handoff's frames 1d, 1e and 1f): beside each marker that
// names a setting, a Reveal button for an Editor and above (disabled with the role it needs for anyone else); a dialog that asks for a
// reason and says the reveal is audited; and, once the server answers, the value in the marker's place with a Hide button and
// "Revealed · recorded in the audit log". The rules of the value are in use-reveal.ts. There is NO copy button for it: a revealed
// value is never copied by this page (plan; design/DEVIATIONS.md, "Copy: a value that is masked").
//
// Only the NFS content of a file can be revealed: `revealValue` reads the file on NFS. So the Git and Baseline tabs, served text and
// docs use MaskedTextView, whose markers carry a token and no control.

type Props = {
  swimlane: string;
  /** `FileContent.path`: what `revealValue` takes with a mask's `setting`. */
  path: string;
  label: string;
  text: string;
  masks: readonly Mask[] | undefined;
  highlightLine?: number | null;
  className?: string;
};

/** The room, in characters of the code font, for the Hide button and "Revealed · recorded in the audit log" after the value. */
const REVEALED_WORDS_CH = 56;

function RevealedValue({ reveal, mark }: { reveal: RevealState; mark: PlannedMark }) {
  const shown = reveal.revealed;
  if (shown === null) return null;
  const setting = visibleText(mark.setting ?? "");
  return (
    <span className="inline-flex items-center gap-2">
      <span data-slot="revealed-value" className="rounded-sm bg-warning-bg px-1 text-foreground">
        <SafeText text={shown.value} />
        {shown.clipped ? <span className="font-sans text-xs text-muted-foreground">{` … first ${MAX_SHOWN_CHARS.toLocaleString("en-US")} characters`}</span> : null}
      </span>
      <Button ref={reveal.hideRef} data-mask-key={mark.key} variant="secondary" aria-label={`Hide value of ${setting}`} className="h-[18px] px-1.5 text-xs" onClick={reveal.hide}>
        Hide
      </Button>
      <span className="font-sans text-xs text-muted-foreground">Revealed · recorded in the audit log</span>
    </span>
  );
}

function RevealButton({ reveal, mark, allowed }: { reveal: RevealState; mark: PlannedMark; allowed: ReturnType<typeof useWriteGate> }) {
  const setting = visibleText(mark.setting ?? "");
  const { consumeRefocus } = reveal;
  return (
    <GatedButton
      gate={allowed}
      variant="secondary"
      aria-label={`Reveal value of ${setting}`}
      className="h-[18px] gap-1 px-1.5 text-xs"
      ref={(button: HTMLButtonElement | null) => {
        // The value was hidden while the Hide button had focus: the Reveal button that takes its place gets it.
        if (button !== null && consumeRefocus(mark.key)) button.focus();
      }}
      onClick={() => reveal.request(mark)}
    >
      <Eye data-icon="inline-start" />
      Reveal
    </GatedButton>
  );
}

export function RevealableMaskedText({ swimlane, path, label, text, masks, highlightLine = null, className }: Props) {
  const gate = useWriteGate("editor");
  const reveal = useReveal({ swimlane, path, text });
  const { revealed } = reveal;


  const renderMask = (mark: PlannedMark, token: ReactNode): ReactNode => {
    if (revealed !== null && revealed.key === mark.key) return <RevealedValue reveal={reveal} mark={mark} />;
    if (mark.setting === null) return token;
    return (
      <>
        {token}
        <RevealButton reveal={reveal} mark={mark} allowed={gate} />
      </>
    );
  };

  return (
    <div className={cn("flex min-h-0 flex-col", className)}>
      <MaskedTextView
        label={label}
        text={text}
        masks={masks}
        // The value, then Hide and "Revealed · recorded in the audit log", take the room of the marker and more: the view is wider by it.
        extraWidthCh={revealed === null ? 0 : revealed.value.length + REVEALED_WORDS_CH}
        highlightLine={highlightLine}
        renderMask={renderMask}
        className="min-h-0 flex-1"
      />
      <p aria-live="polite" className="sr-only">
        {reveal.announcement}
      </p>
      {reveal.target === null || reveal.target.setting === null ? null : (
        <RevealDialog
          setting={reveal.target.setting}
          working={reveal.working}
          error={reveal.error}
          returnFocusRef={reveal.hideRef}
          onConfirm={reveal.confirm}
          onCancel={reveal.cancel}
        />
      )}
    </div>
  );
}
