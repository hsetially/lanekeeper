import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import type { PrPreview, PrTarget } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { prMutations } from "@/features/prs/mutations";

// What the server says a pull request would do for one target: the repository, the branch, the path each file would be
// written to, whether the file is already there, the warnings and the PR that is already open. A preview writes nothing, so it
// is asked as soon as the dialog opens, once for each target it shows and again when the target changes.

export type PrPreviewState = { status: "idle" } | { status: "loading" } | { status: "ready"; preview: PrPreview } | { status: "error"; problem: ApiProblem };

export function usePrPreview(enabled: boolean, lane: string, path: string, target: PrTarget | null): PrPreviewState {
  const client = useQueryClient();
  const mutation = useMutation(prMutations.preview(client));
  const { mutate } = mutation;
  const asked = useRef<string | null>(null);
  const key = target === null ? null : JSON.stringify(target);

  useEffect(() => {
    if (!enabled || target === null || key === null) {
      asked.current = null;
      return;
    }
    if (asked.current === key) return;
    asked.current = key;
    mutate({ swimlane: lane, paths: [path], target });
  }, [enabled, key, target, lane, path, mutate]);

  if (target === null || !enabled) return { status: "idle" };
  if (mutation.isError) return { status: "error", problem: mutation.error };
  if (mutation.isSuccess) return { status: "ready", preview: mutation.data };
  return { status: "loading" };
}
