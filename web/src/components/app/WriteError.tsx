import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { RequestId } from "./RequestId";
import { SafeText } from "./SafeText";

export type WriteErrorInfo = {
  /** What happened. */
  title: string;
  /** Why, and the next step. */
  detail?: string;
  requestId?: string;
};

/**
 * The inline alert a failed write shows inside its dialog: what happened, why, the next step and a copyable
 * request ID. Never a toast (it would disappear) and never the request body.
 */
export function WriteError({ title, detail, requestId }: WriteErrorInfo) {
  return (
    <Alert variant="danger">
      <AlertTitle>
        <SafeText text={title} />
      </AlertTitle>
      {detail ? (
        <AlertDescription>
          <SafeText text={detail} />
        </AlertDescription>
      ) : null}
      {requestId ? <RequestId id={requestId} className="text-danger-fg" /> : null}
    </Alert>
  );
}
