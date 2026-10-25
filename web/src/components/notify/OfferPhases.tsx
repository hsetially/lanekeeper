import type { RefObject } from "react";
import { RequestId } from "@/components/app/RequestId";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { OutcomeDialog } from "./OutcomeDialog";

// The two states of a restart dialog before there is an offer to confirm: it is being read, or it could not be read. Neither can
// restart anything, so neither has a confirm button: a person who sees them can only wait, try the read again, or close.

type Focus = { /** Where focus goes when the dialog closes: the button that started the flow. */ returnFocusRef?: RefObject<HTMLElement | null> };

export function OfferLoadingDialog({ title, description, what, onClose, returnFocusRef }: { title: string; description: string; /** What is being read: "the config-server restart offer". */ what: string; onClose: () => void } & Focus) {
  return (
    <OutcomeDialog open onClose={onClose} tone="info" title={title} description={description} {...(returnFocusRef ? { returnFocusRef } : {})}>
      <div role="status" aria-busy="true" aria-label={`Loading ${what}`} className="flex flex-col gap-2">
        <Skeleton className="h-4 w-3/5" />
        <Skeleton className="h-4 w-4/5" />
        <Skeleton className="h-4 w-2/5" />
      </div>
    </OutcomeDialog>
  );
}

/** `info` says what could not be read and what to do (offerReadFailure). `onRetry` is given only for a problem that a second try can change. */
export function OfferErrorDialog({ info, onRetry, onClose, returnFocusRef }: { info: WriteErrorInfo; onRetry?: () => void; onClose: () => void } & Focus) {
  return (
    <OutcomeDialog
      open
      onClose={onClose}
      tone="warning"
      title={info.title}
      description={info.detail ?? "The offer could not be read."}
      actions={onRetry === undefined ? null : <Button onClick={onRetry}>Retry</Button>}
      {...(returnFocusRef ? { returnFocusRef } : {})}
    >
      {info.requestId === undefined ? null : <RequestId id={info.requestId} />}
    </OutcomeDialog>
  );
}
