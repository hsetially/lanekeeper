import { useCallback, useEffect, useRef, useState } from "react";
import type { ValidationIssue, WriteResult } from "@/api/generated/model";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { classify, errorInfo, type WriteOutcome } from "./outcome";

// What every write dialog shares. The mutation is the colocated one (mutations.ts), so the write is not optimistic and is not
// retried by this layer; this reads its answer. An answer that is a dialog of its own (applied, sent for approval, 409, 423)
// goes to the screen, which owns those dialogs. One that stays here (the content was refused, too large, the request failed)
// is the dialog's inline error, and the person's input stays as it was.

type Mutation<V> = { mutateAsync: (variables: V) => Promise<WriteResult>; isPending: boolean };

export type UseWriteDialog<Vars> = {
  run: (variables: Vars) => Promise<void>;
  working: boolean;
  error: WriteErrorInfo | null;
  /** The issues of a refused content, for the editor to mark (lines and rule names, never text). */
  issues: readonly ValidationIssue[];
  clear: () => void;
};

export function useWriteDialog<Vars>(mutation: Mutation<Vars>, onSettled: (outcome: WriteOutcome) => void): UseWriteDialog<Vars> {
  const [error, setError] = useState<WriteErrorInfo | null>(null);
  const [issues, setIssues] = useState<readonly ValidationIssue[]>([]);
  const latest = useRef(onSettled);
  useEffect(() => {
    latest.current = onSettled;
  });
  const mutate = mutation.mutateAsync;

  const run = useCallback(
    async (variables: Vars): Promise<void> => {
      setError(null);
      setIssues([]);
      let outcome: WriteOutcome;
      try {
        outcome = classify({ result: await mutate(variables) });
      } catch (thrown) {
        outcome = classify({ error: thrown });
      }
      if (outcome.kind === "invalid" || outcome.kind === "too_large" || outcome.kind === "failed") {
        setError(errorInfo(outcome));
        if (outcome.kind === "invalid") setIssues(outcome.issues);
        return;
      }
      latest.current(outcome);
    },
    [mutate],
  );

  const clear = useCallback(() => {
    setError(null);
    setIssues([]);
  }, []);

  return { run, working: mutation.isPending, error, issues, clear };
}
