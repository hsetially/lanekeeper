import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import type { McpToken } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { ErrorState } from "@/components/app/ErrorState";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { PageSkeleton } from "@/components/app/PageSkeleton";
import { SafeText } from "@/components/app/SafeText";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { SecretCopyButton } from "./secret/SecretCopyButton";
import { SecretField } from "./secret/SecretField";
import { accessMutations, afterSecretWrite, createMcpTokenRequest } from "@/features/access/mutations";
import { mcpTokensQuery } from "@/features/access/queries";
import { Key } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { useSecretMutation } from "@/lib/query/secret-mutation";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatClock, formatRelative } from "@/lib/time";

// Personal MCP tokens (S7, S8): the tokens a person made to let their editor's MCP client act as them for seven days. Written
// once and mounted by two screens (S1's My tokens page, S8's Admin "MCP" section, which shows the caller's own tokens), so
// the secret handling exists in one place.
//
// The token the hub returns when one is made is shown ONCE. It is held in this component's state and nowhere else: not in the
// query cache, not in the mutation cache (`useSecretMutation`: gcTime 0, removed and reset in a `finally`), not in storage, not in
// a URL, not in a log. It is cleared when the person says they have stored it and when the component goes. The field is
// read-only and selects the whole token on focus, a click and a double click; beside it is the SecretCopyButton (the human's
// decision of 2026-10-17: a one-time secret has a copy button), which reads the token through a getter at the moment of the click
// and keeps nothing (components/app/secret/SecretCopyButton.tsx). This file and JoinTokenCard are the only two that import it.
//
// The hub answers 404 for the list when the personal-token mode is not switched on: the page says so and shows no form.
//
// There is no handoff design for this screen (the handoff's "My token" is the GitHub token): it is built from the shared
// pieces, and design/DEVIATIONS.md says so.

const MAX_LABEL = 64;
/** The hub's limit (S7): five tokens that work at once. Said in words; the hub is the one that enforces it. */
const LIMIT_TEXT = "You already have the most tokens that work at once (five). Revoke one first.";

type Status = "active" | "expired" | "revoked";

function statusOf(token: McpToken, now: number): Status {
  if (token.revoked_at !== null) return "revoked";
  return token.expires_at <= now ? "expired" : "active";
}

const STATUS_WORD: Record<Status, string> = { active: "Active", expired: "Expired", revoked: "Revoked" };

function problemInfo(error: unknown, fallback: string, byStatus: Partial<Record<number, string>> = {}): WriteErrorInfo {
  if (error instanceof ApiProblem) {
    const detail = byStatus[error.status];
    return { title: error.title, ...(detail === undefined ? {} : { detail }), ...(error.requestId === undefined ? {} : { requestId: error.requestId }) };
  }
  return { title: fallback };
}

function TokenRow({ token, now, onRevoke }: { token: McpToken; now: number; onRevoke: (token: McpToken) => void }) {
  const status = statusOf(token, now);
  return (
    <li className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-border px-3 py-2 last:border-b-0">
      <span className="flex min-w-0 flex-1 items-center gap-2">
        <Key aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        <SafeText text={token.label} className="truncate font-medium" />
      </span>
      <Badge variant={status === "active" ? "outline" : "muted"} className={status === "revoked" ? "border-dashed" : undefined}>
        {STATUS_WORD[status]}
      </Badge>
      <span className="text-xs text-muted-foreground">
        {status === "revoked" ? `Revoked ${formatRelative(token.revoked_at ?? token.created_at, now)}` : status === "expired" ? `Expired ${formatRelative(token.expires_at, now)}` : `Expires ${formatRelative(token.expires_at, now)}`}
      </span>
      <span className="text-xs text-muted-foreground" title={token.last_used_at === null ? undefined : formatClock(token.last_used_at)}>
        {token.last_used_at === null ? "Never used" : `Used ${formatRelative(token.last_used_at, now)}`}
      </span>
      <span className="w-20 text-right">
        {status === "active" ? (
          <Button variant="secondary" size="sm" aria-label={`Revoke ${token.label}`} onClick={() => onRevoke(token)}>
            Revoke
          </Button>
        ) : null}
      </span>
    </li>
  );
}

export function PersonalTokens({
  now = () => Date.now(),
  heading = "Personal MCP tokens",
}: {
  /** The clock, which a test sets. */
  now?: () => number;
  /** The section's heading: a screen whose own title already says "Personal MCP tokens" gives a different one, so a screen reader does not hear it twice. */
  heading?: string;
}) {
  const client = useQueryClient();
  const pages = useCursorPages({ ...mcpTokensQuery(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const make = useSecretMutation(createMcpTokenRequest);
  const revoke = useMutation(accessMutations.revokeMcpToken(client));

  const [label, setLabel] = useState("");
  const [labelError, setLabelError] = useState<string | null>(null);
  const [making, setMaking] = useState(false);
  const [makeError, setMakeError] = useState<WriteErrorInfo | null>(null);
  // The one place the secret lives.
  const [issued, setIssued] = useState<{ id: number; label: string; token: string } | null>(null);
  const [revoking, setRevoking] = useState<McpToken | null>(null);
  const field = useRef<HTMLInputElement>(null);

  // Gone with the component, and gone when the page is put away.
  useEffect(() => {
    const clear = (): void => {
      setIssued(null);
    };
    window.addEventListener("pagehide", clear);
    return () => {
      window.removeEventListener("pagehide", clear);
      clear();
    };
  }, []);

  useEffect(() => {
    if (issued) field.current?.focus();
  }, [issued]);

  const submit = (event: SyntheticEvent): void => {
    event.preventDefault();
    const trimmed = label.trim();
    if (trimmed === "") {
      setLabelError("Give the token a label, such as the name of the machine it is for.");
      return;
    }
    if (trimmed.length > MAX_LABEL) {
      setLabelError(`A label is at most ${MAX_LABEL} characters.`);
      return;
    }
    setLabelError(null);
    setMakeError(null);
    setMaking(true);
    make
      .run({ label: trimmed })
      .then(async (created) => {
        setIssued({ id: created.id, label: created.label, token: created.token });
        setLabel("");
        await afterSecretWrite(client, "createMcpToken");
      })
      .catch((error: unknown) => {
        setMakeError(problemInfo(error, "The token could not be made", { 409: LIMIT_TEXT, 422: "The hub did not accept that label." }));
      })
      .finally(() => {
        setMaking(false);
      });
  };

  // A 404 on the list is the hub saying personal tokens are not switched on: not an error, and no form.
  if (state.state === "error" && state.error?.status === 404) {
    return (
      <EmptyState icon={Key} title="Personal MCP tokens are not switched on">
        The hub is not set up to issue personal tokens, so there is nothing to make or revoke here. Ask an Admin if you need one.
      </EmptyState>
    );
  }

  const nowMs = now();
  return (
    <section aria-labelledby="personal-tokens-title" className="flex flex-col gap-4 p-4">
      <header className="flex flex-col gap-1">
        <h2 id="personal-tokens-title" className="text-base font-semibold">
          {heading}
        </h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          A token lets the MCP client in your editor act as you for seven days. It is shown once, when you make it. You can have five that work at once.
        </p>
      </header>

      {issued ? (
        <Banner
          variant="warning"
          title="Copy your new token now"
          action={
            <Button
              variant="secondary"
              onClick={() => {
                setIssued(null);
              }}
            >
              I have stored it
            </Button>
          }
        >
          It is shown once and cannot be shown again. Copy it with the button, and keep it where only you can read it.
        </Banner>
      ) : null}
      {issued ? (
        <Field>
          <FieldLabel>Your new token</FieldLabel>
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">
              <FieldControl>
                <SecretField ref={field} value={issued.token} />
              </FieldControl>
            </div>
            <SecretCopyButton what="token" source={field} getValue={() => issued.token} />
          </div>
          <FieldDescription>
            For <SafeText text={issued.label} className="font-medium" />. Send it as an Authorization Bearer header.
          </FieldDescription>
        </Field>
      ) : null}

      <form onSubmit={submit} noValidate className="flex flex-col gap-2">
        <Field invalid={labelError !== null} disabled={making}>
          <FieldLabel>Label</FieldLabel>
          <div className="flex items-start gap-2">
            <div className="flex-1">
              <FieldControl>
                <Input
                  value={label}
                  maxLength={MAX_LABEL}
                  autoComplete="off"
                  placeholder="laptop"
                  onChange={(event) => {
                    setLabel(event.target.value);
                    if (labelError) setLabelError(null);
                  }}
                />
              </FieldControl>
            </div>
            <Button type="submit" loading={making}>
              {making ? "Creating…" : "Create token"}
            </Button>
          </div>
          {labelError ? <FieldError>{labelError}</FieldError> : <FieldDescription>What you call it, such as the machine it is for. Not a secret.</FieldDescription>}
        </Field>
        {makeError ? <WriteError {...makeError} /> : null}
      </form>

      {state.state === "loading" ? (
        <PageSkeleton label="Loading personal tokens" rows={3} columns={3} />
      ) : state.state === "error" ? (
        <ErrorState title="Couldn't load personal tokens" {...(state.error ? { problem: state.error } : {})} {...(state.retryable ? { onRetry: state.refetch } : {})} />
      ) : state.state === "empty" ? (
        <EmptyState icon={Key} title="No personal tokens yet">
          Make one above to let your editor's MCP client in.
        </EmptyState>
      ) : (
        <ul aria-label="Personal MCP tokens" className="m-0 list-none overflow-hidden rounded-md border border-border p-0">
          {(state.data ?? []).map((token) => (
            <TokenRow key={token.id} token={token} now={nowMs} onRevoke={setRevoking} />
          ))}
        </ul>
      )}

      <ConsequenceDialog
        open={revoking !== null}
        onOpenChange={(open) => {
          if (!open) {
            setRevoking(null);
            revoke.reset();
          }
        }}
        title={revoking ? `Revoke ${revoking.label}` : "Revoke token"}
        consequence="Any client using this token stops working at once. This cannot be undone."
        actionLabel="Revoke token"
        workingLabel="Revoking…"
        working={revoke.isPending}
        error={revoke.error ? problemInfo(revoke.error, "The token could not be revoked") : null}
        onConfirm={() => {
          if (revoking) revoke.mutate(revoking.id, { onSuccess: () => { setRevoking(null); } });
        }}
      />
    </section>
  );
}
