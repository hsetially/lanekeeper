import { ApiProblem } from "@/api/problem";

// The code-to-state table of the GitHub token screens. The hub refuses a token with a problem whose `type` is the code
// (crates/hub-writes/src/routes/me.rs, and `putGithubToken` in api/openapi.yaml), and each code is ONE state of the screen:
//
//   classic_token, not_fine_grained_token  -> classic   (the policy allows fine-grained tokens only)
//   token_missing_access                   -> repo      (which of the two repositories the token cannot push to)
//   github_account_mismatch                -> mismatch  (the token belongs to another GitHub account than the one linked)
//   idempotency_conflict                   -> retry     (the request clashed with an earlier one; nothing was saved)
//   token_rejected, 503, 400, 403, 429, no answer, anything else -> error (the problem's title and its request id)
//
// A code that is not in the table is an error and never a guess, and the words the screen shows are its own: what the server
// wrote in `detail` is not shown (S21), and the message of an error that is not a problem is never read.

export type ErrorReason = "rejected" | "unavailable" | "bad_request" | "forbidden" | "rate_limited" | "network" | "other";

export type TokenOutcome =
  | { kind: "classic"; problem: ApiProblem }
  | { kind: "repo"; problem: ApiProblem; /** Whether the token can push to the base repository; undefined when the hub did not say. */ base: boolean | undefined; tenant: boolean | undefined }
  | { kind: "mismatch"; problem: ApiProblem }
  | { kind: "retry"; problem: ApiProblem }
  | { kind: "error"; problem: ApiProblem; reason: ErrorReason };

function flag(body: Readonly<Record<string, unknown>>, name: "base" | "tenant"): boolean | undefined {
  const value = body[name];
  return typeof value === "boolean" ? value : undefined;
}

function reasonFor(problem: ApiProblem): ErrorReason {
  if (problem.kind === "token_rejected") return "rejected";
  if (problem.status === 0) return "network";
  if (problem.status === 503 || problem.kind === "unavailable") return "unavailable";
  if (problem.status === 400) return "bad_request";
  if (problem.status === 403) return "forbidden";
  if (problem.status === 429) return "rate_limited";
  return "other";
}

/** What an attempt to store a token ended in, by the problem it was refused with. */
export function outcomeFor(error: unknown): TokenOutcome {
  const problem = error instanceof ApiProblem ? error : new ApiProblem({ status: 0, kind: "unknown", title: "The token could not be saved" });
  switch (problem.kind) {
    case "classic_token":
    case "not_fine_grained_token":
      return { kind: "classic", problem };
    case "token_missing_access":
      return { kind: "repo", problem, base: flag(problem.body, "base"), tenant: flag(problem.body, "tenant") };
    case "github_account_mismatch":
      return { kind: "mismatch", problem };
    case "idempotency_conflict":
      return { kind: "retry", problem };
    default:
      return { kind: "error", problem, reason: reasonFor(problem) };
  }
}
