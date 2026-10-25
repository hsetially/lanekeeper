import { useCallback, useEffect, useId } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

// For a mutation that sends a secret (putGithubToken, createMcpToken, adminIssueJoinToken, a webhook URL) or returns one
// (the one-time value of a token, a revealed setting). A normal mutation keeps its variables and its result in the
// MutationCache for as long as the cache decides, and a screen can read them back from there. For these the value must
// live only where the screen puts it (component state or a ref, cleared on every outcome and on unmount):
//
//  - gcTime 0, and the mutation is removed from the cache in a `finally` and reset, so nothing waits for a timer;
//  - retry false (and src/api/client.ts never retries these paths either: a retry is a second send, or a second audited reveal);
//  - the result goes to the caller through the promise and nowhere else.
//
// Never `setQueryData` or invalidate with a secret, never log it (`no-console`), never put it in a query key or a URL.
export function useSecretMutation<TData, TVariables>(mutationFn: (variables: TVariables) => Promise<TData>) {
  const client = useQueryClient();
  const instance = useId();
  const mutationKey = ["secret-mutation", instance] as const;
  const mutation = useMutation<TData, Error, TVariables>({ mutationKey, mutationFn, gcTime: 0, retry: false, networkMode: "online" });
  const { mutateAsync, reset, isPending } = mutation;

  const forget = useCallback(() => {
    const cache = client.getMutationCache();
    for (const entry of cache.findAll({ mutationKey: ["secret-mutation", instance] })) {
      if (entry.state.status !== "pending") cache.remove(entry);
    }
    reset();
  }, [client, instance, reset]);

  // On unmount: forget now, and again when an in-flight call settles (see run).
  useEffect(() => forget, [forget]);

  const run = useCallback(
    async (variables: TVariables): Promise<TData> => {
      try {
        return await mutateAsync(variables);
      } finally {
        forget();
      }
    },
    [mutateAsync, forget],
  );

  return { run, isPending };
}
