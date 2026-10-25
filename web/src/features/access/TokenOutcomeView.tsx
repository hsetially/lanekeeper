import type { ReactNode } from "react";
import { RepoKind } from "@/api/generated/model";
import { ErrorState } from "@/components/app/ErrorState";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { CircleCheck, CircleX } from "@/icons";
import { REPO_NAMES } from "./repos";
import type { ErrorReason, TokenOutcome } from "./token-outcomes";

// What the screen says for each state of the code-to-state table (./token-outcomes.ts). The words are this screen's: the design's
// for the three states it draws (classic, repository, mismatch), and fixed sentences for the rest. What the server wrote in
// `detail` is never shown. Every state says that nothing was saved, or what to do next; every error carries the request id.

const BASE = REPO_NAMES[RepoKind.base];
const TENANT = REPO_NAMES[RepoKind.tenant];

function RepoRow({ name, ok }: { name: string; ok: boolean }) {
  return (
    <li className="flex items-center gap-2">
      {ok ? <CircleCheck aria-hidden="true" className="text-success-fg" /> : <CircleX aria-hidden="true" className="text-danger-fg" />}
      <code className="font-mono text-mono">{name}</code>
      <span className={ok ? "text-muted-foreground" : "text-danger-fg"}>{ok ? "Read and write OK" : "No access"}</span>
    </li>
  );
}

function Repository({ outcome }: { outcome: Extract<TokenOutcome, { kind: "repo" }> }) {
  const known: Array<{ name: string; ok: boolean }> = [];
  if (outcome.base !== undefined) known.push({ name: BASE, ok: outcome.base });
  if (outcome.tenant !== undefined) known.push({ name: TENANT, ok: outcome.tenant });
  const missing = known.filter((row) => !row.ok).map((row) => row.name);
  // The hub names the repositories by `base` and `tenant`. If it named neither, nothing here claims which one is the problem.
  if (missing.length === 0) {
    return (
      <Alert variant="danger">
        <AlertTitle>This token can't push to both repositories</AlertTitle>
        <AlertDescription className="flex flex-col gap-1.5 text-foreground">
          <span>On GitHub, edit the token, add both repositories under Repository access, then verify again.</span>
          {outcome.problem.requestId ? <RequestId id={outcome.problem.requestId} /> : null}
        </AlertDescription>
      </Alert>
    );
  }
  return (
    <Alert variant="danger">
      <AlertTitle>{`This token can't reach ${missing.join(" or ")}`}</AlertTitle>
      <AlertDescription className="flex flex-col gap-2 text-foreground">
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {known.map((row) => (
            <RepoRow key={row.name} name={row.name} ok={row.ok} />
          ))}
        </ul>
        <span>{`On GitHub, edit the token, add ${missing.join(" and ")} under Repository access, then verify again.`}</span>
        {outcome.problem.requestId ? <RequestId id={outcome.problem.requestId} /> : null}
      </AlertDescription>
    </Alert>
  );
}

function Mismatch({ outcome, linkedLogin }: { outcome: Extract<TokenOutcome, { kind: "mismatch" }>; linkedLogin: string | null }) {
  // The contract's problem carries no login (CG14), so the token's own account is not shown: only the one linked to the person.
  return (
    <Alert variant="danger">
      <AlertTitle>This token belongs to a different GitHub account than the one linked to you</AlertTitle>
      <AlertDescription className="flex flex-col gap-1.5 text-foreground">
        {linkedLogin === null ? null : (
          <dl className="m-0 grid grid-cols-[120px_minmax(0,1fr)] gap-x-2 gap-y-0.5">
            <dt className="text-muted-foreground">Linked to you</dt>
            <dd className="m-0 font-mono text-mono">
              <SafeText text={linkedLogin} />
            </dd>
          </dl>
        )}
        <span>
          {linkedLogin === null ? (
            "Create the token while signed in to the GitHub account that is linked to you."
          ) : (
            <>
              Create the token while signed in to GitHub as <SafeText text={linkedLogin} className="font-mono text-mono" />.
            </>
          )}{" "}
          If the linked account is wrong, ask an admin to change it.
        </span>
        {outcome.problem.requestId ? <RequestId id={outcome.problem.requestId} /> : null}
      </AlertDescription>
    </Alert>
  );
}

const ERROR_TEXT: Record<Exclude<ErrorReason, "rate_limited">, string> = {
  rejected: "GitHub did not accept the token. It may be wrong, expired or revoked. Nothing was saved. Create a new token with the settings above and paste it here.",
  unavailable: "GitHub or the token store did not answer. Nothing was saved. Paste the token and verify again in a moment.",
  bad_request: "The hub could not read that as a token. Nothing was saved. Paste the token again, with nothing before or after it.",
  forbidden: "Saving a GitHub token needs the Editor role. Ask an Admin to change your role.",
  network: "The server could not be reached. Nothing was saved. Check the connection, then paste the token and verify again.",
  other: "The token could not be saved. Nothing was saved.",
};

function errorText(outcome: Extract<TokenOutcome, { kind: "error" }>): string {
  if (outcome.reason !== "rate_limited") return ERROR_TEXT[outcome.reason];
  const wait = outcome.problem.retryAfterSeconds;
  return `Too many attempts. ${wait === undefined ? "Wait a moment" : `Wait ${wait} second${wait === 1 ? "" : "s"}`}, then paste the token and verify again.`;
}

/** The state the screen is in after the hub refused a token (or did not answer). */
export function TokenOutcomeView({ outcome, linkedLogin }: { outcome: TokenOutcome; linkedLogin: string | null }): ReactNode {
  switch (outcome.kind) {
    case "classic":
      return (
        <Alert variant="danger">
          <AlertTitle>Classic tokens aren't allowed by policy</AlertTitle>
          <AlertDescription className="flex flex-col gap-1.5 text-foreground">
            <span>Classic tokens reach every repository you can. Create a fine-grained token with the settings above, then paste it here.</span>
            {outcome.problem.requestId ? <RequestId id={outcome.problem.requestId} /> : null}
          </AlertDescription>
        </Alert>
      );
    case "repo":
      return <Repository outcome={outcome} />;
    case "mismatch":
      return <Mismatch outcome={outcome} linkedLogin={linkedLogin} />;
    case "retry":
      return (
        <Alert variant="danger">
          <AlertTitle>That request clashed with an earlier one</AlertTitle>
          <AlertDescription className="flex flex-col gap-1.5 text-foreground">
            <span>Nothing was saved. Paste the token and verify again.</span>
            {outcome.problem.requestId ? <RequestId id={outcome.problem.requestId} /> : null}
          </AlertDescription>
        </Alert>
      );
    case "error":
      return (
        <ErrorState title="Couldn't verify the token" problem={outcome.problem} className="rounded-md border border-danger-border bg-danger-bg py-6">
          {errorText(outcome)}
        </ErrorState>
      );
  }
}

/** Whether the token itself is what was refused, so that the field is marked invalid. A network error says nothing about the token. */
export function tokenIsAtFault(outcome: TokenOutcome): boolean {
  if (outcome.kind === "classic" || outcome.kind === "repo" || outcome.kind === "mismatch") return true;
  return outcome.kind === "error" && (outcome.reason === "rejected" || outcome.reason === "bad_request");
}
