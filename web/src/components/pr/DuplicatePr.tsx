import type { PrLink } from "@/api/generated/model";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { GitPullRequest, Info } from "@/icons";
import { AppLink } from "@/routes/AppLink";
import { PrCard } from "./PrCard";
import { prAddress } from "./pr-url";

// "An open PR already exists for this exact change: link only, no create button" (Consequence dialogs, frame 1f). The server
// refused a second pull request for the same swimlane, file and content (409 `duplicate_pr`, D77, CG13) and named the open one;
// `duplicatePrOf` (features/prs/duplicate.ts) reads it. The same view serves the preview, which names an open pull request
// (`PrPreview.open_pr`) before anything is asked. Nothing here creates a pull request.

export function DuplicatePr({ link, swimlane }: { link: PrLink; swimlane: string }) {
  const open = link.state === "open";
  const hash = Array.from(link.nfs_hash).slice(0, 12).join("");
  return (
    <div data-slot="duplicate-pr" className="flex flex-col gap-3">
      <Alert variant="info" icon={<Info aria-hidden="true" />}>
        <b className="font-semibold">{open ? `PR #${link.pr_number} is already open for this change.` : `PR #${link.pr_number} is ${link.state}.`}</b>
        <span className="text-muted-foreground">Same target and same content (NFS hash {hash}), so there is nothing new to create.</span>
      </Alert>
      <PrCard link={link} />
      <p className="m-0 text-sm">
        <AppLink route="swimlane-pull-requests" params={{ id: swimlane }} className="underline underline-offset-2">
          All pull requests for {swimlane}
        </AppLink>
      </p>
    </div>
  );
}

/** The dialog footer's action for an existing pull request: "Open PR #418", on GitHub in another tab. Nothing if the address cannot be opened. */
export function OpenPrButton({ link }: { link: PrLink }) {
  const address = prAddress(link.pr_url);
  if (address === null) return null;
  return (
    <Button asChild>
      <a href={address.href} target="_blank" rel="noopener noreferrer">
        <GitPullRequest aria-hidden="true" />
        Open PR #{link.pr_number}
      </a>
    </Button>
  );
}
