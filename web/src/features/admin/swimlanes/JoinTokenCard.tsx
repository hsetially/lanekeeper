import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { SecretCopyButton } from "@/components/app/secret/SecretCopyButton";
import { SecretField } from "@/components/app/secret/SecretField";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { GatedButton } from "@/components/app/GatedButton";
import { Button } from "@/components/ui/button";
import { afterAdminSecretWrite, issueJoinTokenRequest } from "@/features/admin/mutations";
import { adminWriteError } from "@/features/admin/write-error";
import { Key } from "@/icons";
import { useSecretMutation } from "@/lib/query/secret-mutation";
import { formatTime } from "@/lib/time";

// The fallback join token (S5): for a cluster that cannot use Workload Identity. It works for one agent of one swimlane, the hub keeps
// only a hash of it, and it is shown ONCE, here.
//
// The token is a secret, so it lives in this component's state and nowhere else: not in the query cache, not in the mutation cache
// (`useSecretMutation`: gcTime 0, removed and reset in a `finally`, never retried), not in storage, not in a URL, not in a log. It is
// forgotten when the person says they have stored it, when another swimlane is chosen (the card is keyed by swimlane), when the page
// is put away and when the card goes. The field is read-only and selects the whole token on focus, a click and a double click; beside
// it is the SecretCopyButton (the human's decision of 2026-10-17), which reads the token through a getter at the moment of the click
// and keeps nothing (components/app/secret/SecretCopyButton.tsx).

type Issued = { readonly token: string; readonly expiresAt: number };

export function JoinTokenCard({ swimlane }: { swimlane: string }) {
  const client = useQueryClient();
  const issue = useSecretMutation(issueJoinTokenRequest);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [error, setError] = useState<WriteErrorInfo | null>(null);
  const field = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const forget = (): void => {
      setIssued(null);
    };
    window.addEventListener("pagehide", forget);
    return () => {
      window.removeEventListener("pagehide", forget);
      forget();
    };
  }, []);
  useEffect(() => {
    if (issued !== null) field.current?.focus();
  }, [issued]);

  const create = (): void => {
    setError(null);
    setIssued(null);
    issue
      .run(swimlane)
      .then(async (created) => {
        setIssued({ token: created.token, expiresAt: created.expires_at });
        await afterAdminSecretWrite(client, "adminIssueJoinToken");
      })
      .catch((failure: unknown) => {
        setError(adminWriteError(failure, "The join token could not be made"));
      });
  };

  return (
    <div className="flex flex-col gap-2">
      <h4 className="text-sm font-semibold">Fallback join token</h4>
      <p className="max-w-prose text-sm text-muted-foreground">Agents normally join with Workload Identity. A join token is the fallback: it works for one agent, it is shown once and it expires.</p>
      {issued === null ? null : (
        <div role="status" className="flex flex-col gap-2 rounded-md border border-warning-border bg-warning-bg p-3">
          <div className="flex items-center gap-2 text-sm font-semibold text-warning-fg">
            <Key aria-hidden="true" className="size-4" />
            Join token created · shown once
          </div>
          <p className="text-sm text-pretty">
            Copy it now. Lanekeeper keeps only a hash, so this token can't be shown again. It works for one agent in <b className="font-mono">{swimlane}</b> and expires {formatTime(issued.expiresAt)}.
          </p>
          <div className="flex items-start gap-2">
            <SecretField ref={field} aria-label="Your join token" value={issued.token} className="min-w-0 flex-1" />
            <SecretCopyButton what="join token" source={field} getValue={() => issued.token} />
          </div>
          <div>
            <Button
              size="sm"
              onClick={() => {
                setIssued(null);
              }}
            >
              I have stored it
            </Button>
          </div>
        </div>
      )}
      <div>
        <GatedButton minRole="admin" variant="secondary" loading={issue.isPending} onClick={create}>
          <Key data-icon="inline-start" />
          Create join token
        </GatedButton>
      </div>
      {error ? <WriteError {...error} /> : null}
    </div>
  );
}
