import { useState } from "react";
import type { GithubCredential } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { PageProps } from "@/routes/props";
import { AppLink } from "@/routes/AppLink";
import { TokenSaved } from "./TokenSaved";
import { TokenSetup } from "./TokenSetup";

// The GitHub token onboarding (the Access Screen's "token" frames, 1g to 1r): the first thing an Editor or above sees when no
// token is stored (`/` sends them here when `needs_github_token` is true; nothing else does, so the rest of the app stays
// readable). The route needs the Editor role, so a Viewer gets the permission state from the route gate and no menu entry.
//
// The design draws this as a single focused page with a header of its own (name, role, Sign out). The shell draws the same frame
// around every signed-in route, so this page is the content of that frame, and design/DEVIATIONS.md says so.
export default function OnboardingPage(_props: PageProps<"onboarding">) {
  const session = useSession();
  const me = session.status === "ready" ? session.me : null;
  const [saved, setSaved] = useState<GithubCredential | null>(null);

  return (
    <div className="flex justify-center px-6 pt-8 pb-8">
      <div className="flex w-full max-w-[640px] flex-col gap-5">
        <div className="flex flex-col gap-1.5">
          <h1 className="text-2xl font-semibold tracking-tight">Add your GitHub token</h1>
          <p className="text-base text-muted-foreground">Lanekeeper raises pull requests as you, so it needs a fine-grained token that you create. It's stored encrypted and never shown again.</p>
        </div>

        {saved ? (
          <TokenSaved credential={saved} />
        ) : (
          <>
            {me?.needs_github_token === false ? (
              <Alert variant="info">
                <AlertDescription>
                  You already have a GitHub token stored. Saving a new one here replaces it. To see or delete the stored one, open{" "}
                  <AppLink route="github-token" className="font-medium text-link underline underline-offset-2 hover:text-primary-hover">
                    My GitHub token
                  </AppLink>
                  .
                </AlertDescription>
              </Alert>
            ) : null}
            <TokenSetup linkedLogin={me?.github_login ?? null} onStored={setSaved} />
          </>
        )}
      </div>
    </div>
  );
}
