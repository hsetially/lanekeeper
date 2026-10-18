import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useId, useRef, useState, type ChangeEvent, type ReactNode, type SyntheticEvent } from "react";
import { RepoKind, type GithubCredential } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { useWriteGate } from "@/components/app/write-gate";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { useSecretMutation } from "@/lib/query/secret-mutation";
import { cn } from "@/lib/utils";
import { afterSecretWrite, putGithubTokenRequest } from "./mutations";
import { GITHUB_TOKEN_SETTINGS_URL, MAX_TOKEN_LENGTH, REPO_NAMES } from "./repos";
import { TokenOutcomeView, tokenIsAtFault } from "./TokenOutcomeView";
import { outcomeFor, type TokenOutcome } from "./token-outcomes";

// Where a person gives Lanekeeper their GitHub token: what to create on GitHub (the design's step 1), and the masked field that
// the token is pasted into (step 2). Used by the onboarding page and by "My GitHub token" (to add or replace a token).
//
// The token is a SECRET (S2, S7, S21). It lives in this component's state and nowhere else:
//  - it is typed into a real `<input type="password">` (not a styled span), which the browser is told not to remember or check;
//  - it goes to the hub through `useSecretMutation` (gcTime 0, removed from the mutation cache in a `finally`, never retried; the
//    fetch layer does not resend it either), with the CSRF token and a fresh Idempotency-Key;
//  - the field is emptied after EVERY answer (success, refusal, no answer), when the page is put away, and when the component
//    goes. After a refusal the person pastes again: the screen never keeps a token it has already sent;
//  - it is never put in a query key, a URL, a cache, a log or an error message.
//
// "Verifying" is one honest line: the hub checks the token type, the access to both repositories and the linked login in a single
// request and answers once, so the screen has no steps to count down and shows none (design/DEVIATIONS.md).

type Phase = { kind: "idle" } | { kind: "verifying" } | { kind: "empty" } | { kind: "too_long" } | { kind: "outcome"; outcome: TokenOutcome };

const DT = "px-3 py-2 text-muted-foreground";
const DD = "m-0 px-3 py-2";

function Row({ term, last = false, className, children }: { term: string; last?: boolean; className?: string; children: ReactNode }) {
  return (
    <>
      <dt className={cn(DT, !last && "border-b border-border")}>{term}</dt>
      <dd className={cn(DD, !last && "border-b border-border", className)}>{children}</dd>
    </>
  );
}

function RepoChip({ name }: { name: string }) {
  return <code className="inline-flex h-[22px] items-center rounded-sm border border-border bg-muted px-1.5 font-mono text-mono">{name}</code>;
}

/** Step 1: what the token must be. */
export function TokenInstructions() {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="flex flex-col rounded-md border border-border bg-card">
      <div className="flex h-9 items-center justify-between gap-3 border-b border-border px-3">
        <h2 id={headingId} className="text-sm font-semibold">
          1 · Create the token on GitHub with these settings
        </h2>
        <a href={GITHUB_TOKEN_SETTINGS_URL} target="_blank" rel="noopener noreferrer" className="text-xs font-medium text-link hover:text-primary-hover">
          Open token settings<span className="sr-only"> (opens GitHub in a new tab)</span>
        </a>
      </div>
      <dl className="m-0 grid grid-cols-[150px_minmax(0,1fr)] text-sm">
        <Row term="Token type">
          <strong className="font-semibold">Fine-grained</strong> only. Classic tokens are rejected.
        </Row>
        <Row term="Repositories" className="flex flex-wrap items-center gap-1.5">
          <span className="text-muted-foreground">Only select repositories:</span>
          <RepoChip name={REPO_NAMES[RepoKind.base]} />
          <RepoChip name={REPO_NAMES[RepoKind.tenant]} />
        </Row>
        <Row term="Permissions" className="flex flex-col gap-0.5">
          <span>
            Contents: <strong className="font-semibold">Read and write</strong>
          </span>
          <span>
            Pull requests: <strong className="font-semibold">Read and write</strong>
          </span>
        </Row>
        <Row term="Expiration" last>
          Set an expiry date. Lanekeeper warns you 7 days before it ends.
        </Row>
      </dl>
    </section>
  );
}

type TokenSetupProps = {
  /** The GitHub login linked to the person, if any: the mismatch state names it. */
  linkedLogin: string | null;
  /** The hub stored the token. Nothing of the token is passed. */
  onStored: (credential: GithubCredential) => void;
  /** Put the cursor in the field when the form appears (it was opened by a button). */
  focusOnMount?: boolean;
  className?: string;
};

export function TokenSetup({ linkedLogin, onStored, focusOnMount = false, className }: TokenSetupProps) {
  const client = useQueryClient();
  const put = useSecretMutation(putGithubTokenRequest);
  // Saving a token is a write: it needs the Editor role and the live connection (disabled with the reason, never hidden).
  const gate = useWriteGate("editor");
  const [token, setToken] = useState("");
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const input = useRef<HTMLInputElement>(null);
  const mounted = useRef(false);
  const fieldId = useId();
  const hintId = useId();
  const answerId = useId();

  // Only when the form opens: the person's place in it is theirs after that.
  const focusFirst = useRef(focusOnMount);
  useEffect(() => {
    if (focusFirst.current) input.current?.focus();
  }, []);

  useEffect(() => {
    mounted.current = true;
    // The page is put away (a tab discarded, the back-forward cache): the token goes with it.
    const clear = (): void => {
      setToken("");
    };
    window.addEventListener("pagehide", clear);
    return () => {
      mounted.current = false;
      window.removeEventListener("pagehide", clear);
    };
  }, []);

  // After any answer the person is back in the field, ready to paste another.
  const answered = phase.kind !== "idle" && phase.kind !== "verifying";
  useEffect(() => {
    if (answered) input.current?.focus();
  }, [answered, phase]);

  const refuse = useCallback((next: Phase): void => {
    setToken("");
    setPhase(next);
  }, []);

  const submit = (event: SyntheticEvent): void => {
    event.preventDefault();
    if (phase.kind === "verifying" || !gate.allowed) return;
    const value = token.trim();
    if (value === "") {
      refuse({ kind: "empty" });
      return;
    }
    if (value.length > MAX_TOKEN_LENGTH) {
      refuse({ kind: "too_long" });
      return;
    }
    setPhase({ kind: "verifying" });
    void put.run({ token: value }).then(
      (credential) => {
        setToken("");
        // Whatever the page does next, the rest of the app learns that a token is stored (the user's `needs_github_token`, the summary).
        void afterSecretWrite(client, "putGithubToken").catch(() => undefined);
        if (!mounted.current) return;
        setPhase({ kind: "idle" });
        onStored(credential);
      },
      (error: unknown) => {
        setToken("");
        if (mounted.current) setPhase({ kind: "outcome", outcome: outcomeFor(error) });
      },
    );
  };

  const change = (event: ChangeEvent<HTMLInputElement>): void => {
    setToken(event.target.value);
    // Starting again clears the earlier answer.
    if (phase.kind !== "idle" && phase.kind !== "verifying") setPhase({ kind: "idle" });
  };

  const verifying = phase.kind === "verifying";
  const invalid = phase.kind === "empty" || phase.kind === "too_long" || (phase.kind === "outcome" && tokenIsAtFault(phase.outcome));
  const hasAnswer = phase.kind === "empty" || phase.kind === "too_long" || phase.kind === "outcome";

  return (
    <div className={cn("flex flex-col gap-5", className)}>
      <TokenInstructions />
      <form onSubmit={submit} noValidate className="flex flex-col gap-2">
        <Label htmlFor={fieldId} className="font-semibold">
          2 · Paste the token
        </Label>
        <div className="flex items-center gap-2">
          <Input
            ref={input}
            id={fieldId}
            type="password"
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            data-1p-ignore
            data-lpignore="true"
            placeholder="github_pat_…"
            value={token}
            readOnly={verifying}
            onChange={change}
            aria-invalid={invalid ? true : undefined}
            aria-describedby={hasAnswer ? `${hintId} ${answerId}` : hintId}
            aria-busy={verifying ? true : undefined}
            className={cn("h-8 flex-1 font-mono text-mono", verifying && "bg-muted")}
          />
          <GatedButton type="submit" minRole="editor" className="h-8 px-3" loading={verifying}>
            {verifying ? "Verifying…" : "Verify and save"}
          </GatedButton>
        </div>
        <p id={hintId} className="text-xs text-muted-foreground">
          The token is masked as you paste it. After saving, only your GitHub login and the expiry date are shown.
        </p>

        {verifying ? (
          <div role="status" className="flex items-start gap-2 rounded-md border border-border bg-muted px-3 py-2.5">
            <Spinner className="mt-0.5" />
            <span>Verifying with GitHub. This checks the token type, its access to both repositories and the GitHub login linked to you, in one request.</span>
          </div>
        ) : null}

        <div id={answerId}>
          {phase.kind === "empty" ? (
            <p role="alert" className="text-xs text-danger-fg">
              Paste the token first.
            </p>
          ) : null}
          {phase.kind === "too_long" ? (
            <p role="alert" className="text-xs text-danger-fg">
              That is longer than a GitHub token can be. Paste only the token, with nothing before or after it.
            </p>
          ) : null}
          {phase.kind === "outcome" ? <TokenOutcomeView outcome={phase.outcome} linkedLogin={linkedLogin} /> : null}
        </div>
      </form>
    </div>
  );
}
