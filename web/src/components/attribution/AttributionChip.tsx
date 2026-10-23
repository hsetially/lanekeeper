import type { Attribution } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { SafeText } from "@/components/app/SafeText";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { CircleHelp } from "@/icons";
import { cn } from "@/lib/utils";
import { AttributionEvidence } from "./AttributionEvidence";
import { ConfidenceMeter } from "./ConfidenceMeter";
import { describeAttribution, type NameLookup } from "./model";

// Who changed a file, how Lanekeeper knows, and how sure it is, in one line: "Marco Diaz via Lanekeeper  [bars] Certain". For a
// history entry, a drift row, an activity row and the head of a file. A press (or Enter) opens the evidence behind it; the
// control is at least 24 px high (WCAG 2.5.8) though the chip inside it is 18.
//
// What it will not do: name a person the contract does not name (a user is an id, so it is "Lanekeeper user" and the start of the
// id unless the screen gives a lookup), show a confidence the server did not give (the unknown source has none), or draw
// untrusted text raw (a login, a job name and a user's name are all SafeText, so a bidi override or a zero-width character is a
// visible marker).

export type AttributionChipProps = {
  /** What the hub sent; null when nothing is recorded (a file never observed, an event with no attribution). */
  attribution: Attribution | null;
  /** The signed-in person's id, if the screen knows it. Default: the session's. `null`: nobody is "You". */
  you?: string | null | undefined;
  /** The name of a user, if the screen has the user list (the Admin screens do). Without it a user is "Lanekeeper user" and a short id. */
  nameOf?: NameLookup | undefined;
  /** Open the evidence on a press (the default). Off: plain text, for a row that opens a drawer of its own. */
  evidence?: boolean;
  /** What to say when `attribution` is null. */
  emptyLabel?: string;
  className?: string;
};

export function AttributionChip({ attribution, you, nameOf, evidence = true, emptyLabel = "Not recorded", className }: AttributionChipProps) {
  const session = useSession();
  const meId = you !== undefined ? you : session.status === "ready" ? session.me.id : null;
  if (attribution === null) {
    return (
      <span data-slot="attribution" data-known="false" data-who="none" className={cn("text-xs text-muted-foreground", className)}>
        {emptyLabel}
      </span>
    );
  }
  const view = describeAttribution(attribution, { you: meId, nameOf });
  const unknown = view.who.kind === "unknown";
  // The name and the words after it are ONE run of text, as the design draws them (`<span>name <span>· via Lanekeeper</span></span>`): a
  // space between them, and the run gives way with an ellipsis, so the chip is that run and the score beside it, on one line or on
  // two, never three. A login, a job or a name can be 256 characters, and the words after it ("(the file's owner, a hint)") are long
  // too: as separate items of a wrapping line they were three lines in rows built for two, which ran over the links under them. The
  // name has a width of its own (12rem) so that a long one leaves the words in sight; the evidence has both whole.
  const body = (
    <>
      <span data-slot="attribution-text" className={cn("block min-w-0 max-w-full truncate", unknown && "text-muted-foreground italic")}>
        {unknown ? <CircleHelp aria-hidden="true" className="mr-1 inline size-3 flex-none align-[-1px]" /> : null}
        {/* The name is the 500-weight part; the space after it is the run's own (weight 400), as in the design: in the 500-weight run it is 0.16 px narrower. */}
        <SafeText text={view.who.label} className="inline-block max-w-48 truncate align-top font-medium" />
        {view.how === "" ? null : (
          <>
            {" "}
            <span className="text-muted-foreground">
              {view.dotted ? <span aria-hidden="true">· </span> : null}
              {view.how}
            </span>
          </>
        )}
      </span>
      <ConfidenceMeter confidence={view.confidence.level} />
    </>
  );
  const layout = "inline-flex max-w-full flex-wrap items-center gap-x-2 gap-y-1.5 text-xs leading-4";
  return (
    <span data-slot="attribution" data-known={view.known} data-who={view.who.kind} data-source={view.source} className={cn("inline-flex max-w-full", className)}>
      {evidence ? (
        <Popover>
          <PopoverTrigger asChild>
            <button type="button" className={cn(layout, "min-h-6 cursor-pointer rounded-sm text-left hover:bg-accent")}>
              {body}
            </button>
          </PopoverTrigger>
          <PopoverContent aria-label="Attribution evidence" className="w-80">
            <AttributionEvidence attribution={attribution} you={meId} nameOf={nameOf} />
          </PopoverContent>
        </Popover>
      ) : (
        <span className={layout}>{body}</span>
      )}
    </span>
  );
}
