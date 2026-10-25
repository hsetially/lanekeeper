import type { ReactNode } from "react";
import { useSignOut } from "@/auth/signout";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { SafeText } from "@/components/app/SafeText";
import { cn } from "@/lib/utils";

// The frame of the screens before sign-in (the design's Access Screen, frames 1a to 1f): a card in the middle of a plain
// background, with no navigation at all. The shell draws nothing around a public route; the page provides this and an h1.

/** The card. The design's 400 px wide card with 32 px of padding and 20 px between its parts. */
export function AccessFrame({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-1 items-center justify-center bg-muted p-6">
      <div className="flex w-[400px] max-w-full flex-col gap-5 rounded-md border border-border bg-card p-8 text-card-foreground">{children}</div>
    </div>
  );
}

/** The part of a card that says what the screen is: a badge, the h1 and one paragraph. */
export function AccessHeading({ badge, title, children, className }: { badge: ReactNode; title: string; children: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-3", className)}>
      <div className="flex self-start">{badge}</div>
      <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
      <p className="text-base text-muted-foreground">{children}</p>
    </div>
  );
}

/**
 * Sign out, for a person who is signed in but cannot use the app yet (pending) or any more (disabled). The request is made by
 * fetch with the CSRF token (a form navigation would carry `Origin: null` and be refused), and the person is told if the hub
 * does not confirm it.
 */
export function SignOutLink() {
  const { signOut, pending, problem } = useSignOut();
  return (
    <div className="flex flex-col gap-2">
      <button
        type="button"
        disabled={pending}
        onClick={() => {
          void signOut();
        }}
        className="cursor-pointer self-start bg-transparent p-0 text-sm font-medium text-link hover:text-primary-hover hover:underline disabled:cursor-progress disabled:opacity-60"
      >
        Sign out
      </button>
      {problem ? (
        <Alert variant="danger">
          <AlertTitle>Could not sign out</AlertTitle>
          <AlertDescription>
            <SafeText text={problem.title} />
          </AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}
