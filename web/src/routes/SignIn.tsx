import { useSearch } from "@tanstack/react-router";
import { loginHref } from "@/auth/session";
import { Button } from "@/components/ui/button";

// Sign-in is a GET navigation to the hub's own login endpoint (S1: Entra does the rest). The link carries the validated
// `next` as `return_to`; an invalid `next` is dropped by the route's search validator before it gets here. The designed
// screen (the handoff's Access Screen) is built in T4 and replaces this one.
export function SignIn() {
  const search: { next?: string } = useSearch({ strict: false });
  return (
    <main className="flex flex-col items-start gap-3 p-6">
      <h1 className="text-lg font-semibold">Sign in</h1>
      <p className="text-muted-foreground">Use your work account.</p>
      <Button asChild>
        <a href={loginHref(search.next)}>Sign in with Microsoft</a>
      </Button>
    </main>
  );
}
