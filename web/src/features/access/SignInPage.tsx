import { loginHref } from "@/auth/session";
import { Button } from "@/components/ui/button";
import type { PageProps } from "@/routes/props";
import { AccessFrame } from "./AccessFrame";

// The Access Screen's sign-in (frames 1a and 1b). Sign-in is a GET navigation to the hub's own login endpoint (S1: Entra does
// the rest), so this is a link, not a form and not a request of the app: a public route asks the hub for nothing and opens no
// event stream. The link carries the page the person wanted as `return_to`; the route's search validator already dropped a
// `next` that is not a same-origin path of the app, and `loginHref` checks it once more.
export default function SignInPage({ search }: PageProps<"signin">) {
  return (
    <AccessFrame>
      <div className="flex flex-col gap-1.5">
        <h1 className="text-2xl font-semibold tracking-tight">Lanekeeper</h1>
        <p className="text-base text-muted-foreground">Review and edit configuration across the SIT and PRESIT swimlanes.</p>
      </div>
      <Button asChild className="h-8 w-full text-base">
        <a href={loginHref(search.next)}>Sign in with Microsoft</a>
      </Button>
    </AccessFrame>
  );
}
