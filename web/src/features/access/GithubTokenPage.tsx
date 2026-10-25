import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { GithubCredential } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { Banner } from "@/components/app/Banner";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { useWriteGate } from "@/components/app/write-gate";
import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Clock, CircleX, Key, RefreshCw, Trash2 } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useNow } from "@/live/use-now";
import type { PageProps } from "@/routes/props";
import { useQueryState } from "@/lib/query/use-query-state";
import { expiryWords, tokenHealth, type TokenHealth } from "./expiry";
import { accessMutations } from "./mutations";
import { githubCredentialQuery } from "./queries";
import { writeErrorInfo } from "./problem-info";
import { ExpiryText } from "./TokenSaved";
import { TokenSetup } from "./TokenSetup";

// "My GitHub token" (the Access Screen's "mytoken" frames, 1s and 1t): what the hub stored of the person's token (its login, when
// it ends, what was verified), never the token. A token that is about to end, or has ended, is said at the top of the page with
// the way to replace it (W32); replacing it and deleting it are done here.
//
// What the design draws and the contract cannot give is left out: when the token was added and when it was last used, and the
// per-repository permission names (design/DEVIATIONS.md, CG14). The hub does say what it verified (`scopes`: a push to each of
// the two repositories) and when, and the page shows that instead.

type Notice = "stored" | "replaced" | "deleted";

const NOTICE: Record<Notice, { variant: "success" | "info"; title: string; text?: string }> = {
  stored: { variant: "success", title: "Token verified and saved", text: "Save and Raise PR can use it now. The token itself cannot be shown again." },
  replaced: { variant: "success", title: "Token replaced", text: "The old token is no longer used. The new one cannot be shown again." },
  deleted: { variant: "info", title: "Your GitHub token was deleted.", text: "Add a new one to save and raise pull requests again." },
};

function Item({ term, last = false, children }: { term: string; last?: boolean; children: ReactNode }) {
  return (
    <>
      <dt className={cn("px-3 py-2 text-muted-foreground", !last && "border-b border-border")}>{term}</dt>
      <dd className={cn("m-0 px-3 py-2", !last && "border-b border-border")}>{children}</dd>
    </>
  );
}

/** What the hub verified, as it sent it: `push:owner/repo` is "Push access" to that repository. */
function Verified({ scopes }: { scopes: readonly string[] }) {
  if (scopes.length === 0) return <span className="text-muted-foreground">The hub did not report what was verified.</span>;
  return (
    <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
      {scopes.map((scope, i) => {
        const push = scope.startsWith("push:");
        return (
          <li key={`${i}:${scope}`}>
            <code className="font-mono text-mono">
              <SafeText text={push ? scope.slice("push:".length) : scope} />
            </code>
            {push ? <span className="text-muted-foreground"> · Push access</span> : null}
          </li>
        );
      })}
    </ul>
  );
}

function HealthBadge({ health, credential, now }: { health: TokenHealth; credential: GithubCredential; now: number }) {
  if (health === "ok" || credential.expires_at === null || credential.expires_at === undefined) return null;
  if (health === "expired") {
    return (
      <Badge className="border-danger-border bg-danger-bg text-danger-fg">
        <CircleX aria-hidden="true" />
        Expired
      </Badge>
    );
  }
  return (
    <Badge className="border-warning-border bg-warning-bg text-warning-fg">
      <Clock aria-hidden="true" />
      Expires {expiryWords(credential.expires_at, now)}
    </Badge>
  );
}

function ExpiryBanner({ health, credential, now, onReplace, replacing }: { health: TokenHealth; credential: GithubCredential; now: number; onReplace: () => void; replacing: boolean }) {
  if (health === "ok" || credential.expires_at === null || credential.expires_at === undefined) return null;
  const action = (
    <Button variant="secondary" size="sm" aria-expanded={replacing} onClick={onReplace}>
      Replace token
    </Button>
  );
  if (health === "expired") {
    return (
      <Banner variant="danger" title="Your GitHub token has expired" action={action}>
        Save and Raise PR don't work until you replace it.
      </Banner>
    );
  }
  return (
    <Banner variant="warning" title={`Your GitHub token expires ${expiryWords(credential.expires_at, now)}`} action={action}>
      After {formatTime(credential.expires_at)}, Save and Raise PR stop working.
    </Banner>
  );
}

function Summary({ credential, health, now }: { credential: GithubCredential; health: TokenHealth; now: number }) {
  return (
    <dl className="m-0 grid grid-cols-[180px_minmax(0,1fr)] overflow-hidden rounded-md border border-border bg-card">
      <Item term="Linked GitHub login">
        <span className="font-mono text-mono font-medium">
          <SafeText text={credential.github_login} />
        </span>
      </Item>
      <Item term="Expires">
        <span className={cn("tabular-nums", health !== "ok" && "font-medium text-warning-fg")}>
          <ExpiryText expiresAt={credential.expires_at} now={now} />
        </span>
      </Item>
      <Item term="Verified">
        <span className="tabular-nums">
          {formatTime(credential.verified_at)} <span className="text-muted-foreground">· {formatRelative(credential.verified_at, now)}</span>
        </span>
      </Item>
      <Item term="Verified access">
        <Verified scopes={credential.scopes} />
      </Item>
      <Item term="Token" last>
        <span className="text-muted-foreground">Stored encrypted. It can't be viewed or copied.</span>
      </Item>
    </dl>
  );
}

function SummarySkeleton() {
  return (
    <div data-slot="token-skeleton" className="flex flex-col gap-2 rounded-md border border-border p-3">
      {[0, 1, 2, 3, 4].map((i) => (
        <Skeleton key={i} className="h-4 w-full" />
      ))}
    </div>
  );
}

export default function GithubTokenPage(_props: PageProps<"github-token">) {
  const client = useQueryClient();
  const session = useSession();
  const linkedLogin = session.status === "ready" ? session.me.github_login : null;
  const credential = useQuery(githubCredentialQuery());
  const state = useQueryState(credential);
  const now = useNow(true, 60_000);
  const remove = useMutation(accessMutations.deleteGithubToken(client));
  const gate = useWriteGate("editor");

  const [replacing, setReplacing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  // Between the hub storing a first token and the page learning it.
  const [justStored, setJustStored] = useState(false);

  // A 404 is the hub saying that no token is stored: not an error. It is read from the query itself, because a refetch that
  // answers 404 (after a delete) keeps the old data in the result and an error beside it.
  const none = credential.status === "error" && credential.error.status === 404;
  useEffect(() => {
    if (credential.isSuccess) setJustStored(false);
  }, [credential.isSuccess]);

  // When the dialog closes it gives the focus back to the button that opened it (src/components/ui/return-focus.ts). When the token
  // was deleted the button is gone, and the focus goes to the page's h1.
  const deleteButton = useRef<HTMLButtonElement>(null);
  const title = useRef<HTMLHeadingElement>(null);
  const wasDeleting = useRef(false);
  useEffect(() => {
    if (deleting) {
      wasDeleting.current = true;
      return;
    }
    if (!wasDeleting.current) return;
    wasDeleting.current = false;
    if (deleteButton.current === null) title.current?.focus();
  }, [deleting]);

  const openReplace = (): void => {
    setNotice(null);
    setReplacing(true);
  };

  const data = none ? undefined : credential.data;
  const health = data ? tokenHealth(data, now) : "ok";

  return (
    <div className="flex flex-col">
      {data ? <ExpiryBanner health={health} credential={data} now={now} onReplace={openReplace} replacing={replacing} /> : null}
      <div className="px-6 py-6">
        <div className="flex max-w-[720px] flex-col gap-5">
          <div className="flex items-start justify-between gap-4">
            <div className="flex flex-col gap-1.5">
              <h1 ref={title} tabIndex={-1} className="text-xl font-semibold tracking-tight">
                My GitHub token
              </h1>
              <p className="text-muted-foreground">Used to raise pull requests as you in configuration-base-saas and csp-tenant-data.</p>
            </div>
            {data ? <HealthBadge health={health} credential={data} now={now} /> : null}
          </div>

          {notice ? (
            <Alert variant={NOTICE[notice].variant}>
              <AlertTitle>{NOTICE[notice].title}</AlertTitle>
              {NOTICE[notice].text ? <AlertDescription>{NOTICE[notice].text}</AlertDescription> : null}
            </Alert>
          ) : null}

          {none ? (
            justStored ? (
              <SummarySkeleton />
            ) : (
              <>
                <EmptyState icon={Key} title="No GitHub token stored" className="rounded-md border border-border">
                  Lanekeeper raises pull requests as you, so it needs a fine-grained token that you create. Until you add one, you can't save or raise PRs.
                </EmptyState>
                <TokenSetup
                  linkedLogin={linkedLogin}
                  onStored={() => {
                    setJustStored(true);
                    setNotice("stored");
                  }}
                />
              </>
            )
          ) : (
            <QueryBoundaryView state={state} noun="your GitHub token" loading={<SummarySkeleton />}>
              {(stored) => (
                <div className="flex flex-col gap-5">
                  <Summary credential={stored} health={tokenHealth(stored, now)} now={now} />
                  <div className="flex flex-wrap items-center gap-2">
                    <Button aria-expanded={replacing} onClick={replacing ? () => { setReplacing(false); } : openReplace}>
                      <RefreshCw aria-hidden="true" />
                      Replace token
                    </Button>
                    <GatedButton
                      ref={deleteButton}
                      minRole="editor"
                      variant="secondary"
                      className="border-danger-border text-danger-fg hover:bg-danger-bg"
                      onClick={() => {
                        remove.reset();
                        setDeleting(true);
                      }}
                    >
                      <Trash2 aria-hidden="true" />
                      Delete token
                    </GatedButton>
                    <span className="ml-2 text-xs text-muted-foreground">Deleting means you can't save or raise PRs until you add a new one.</span>
                  </div>
                  {replacing ? (
                    <div className="flex flex-col gap-3 border-t border-border pt-5">
                      <TokenSetup
                        focusOnMount
                        linkedLogin={linkedLogin}
                        onStored={() => {
                          setReplacing(false);
                          setNotice("replaced");
                        }}
                      />
                      <Button
                        variant="secondary"
                        className="self-start"
                        onClick={() => {
                          setReplacing(false);
                        }}
                      >
                        Cancel
                      </Button>
                    </div>
                  ) : null}
                </div>
              )}
            </QueryBoundaryView>
          )}
        </div>
      </div>

      <ConsequenceDialog
        open={deleting}
        onOpenChange={(open) => {
          if (!open) {
            setDeleting(false);
            remove.reset();
          }
        }}
        title="Delete your GitHub token"
        consequence="Lanekeeper forgets the stored token. You can't save or raise PRs until you add a new token, and this one cannot be recovered."
        actionLabel="Delete token"
        workingLabel="Deleting…"
        working={remove.isPending}
        canConfirm={gate.allowed}
        error={remove.error ? writeErrorInfo(remove.error, "The token could not be deleted") : null}
        onConfirm={() => {
          remove.mutate(undefined, {
            onSuccess: () => {
              setDeleting(false);
              setReplacing(false);
              setNotice("deleted");
            },
          });
        }}
      />
    </div>
  );
}
