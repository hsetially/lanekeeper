import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { apiCaches } from "@/api/client";
import { authLogout } from "@/api/generated/endpoints/auth/auth";
import { ApiProblem } from "@/api/problem";
import type { AppRouter } from "@/router";

/**
 * Sign out: `POST /auth/logout` by fetch (a <form method=post> navigation would carry `Origin: null` and the CSRF layer
 * would refuse it), then empty every cache and go to /signin. If the hub does not confirm, the person stays where they
 * are and is told: on a shared machine "signed out" must mean the session is really gone.
 */
export function useSignOut() {
  const queryClient = useQueryClient();
  const router = useRouter<AppRouter>();
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<ApiProblem | null>(null);

  const signOut = useCallback(async (): Promise<boolean> => {
    setPending(true);
    setProblem(null);
    try {
      await authLogout();
    } catch (error) {
      setProblem(error instanceof ApiProblem ? error : new ApiProblem({ status: 0, kind: "unknown", title: "Could not sign out" }));
      setPending(false);
      return false;
    }
    queryClient.clear();
    apiCaches.clear();
    setPending(false);
    router.history.replace("/signin");
    return true;
  }, [queryClient, router]);

  return { signOut, pending, problem };
}
