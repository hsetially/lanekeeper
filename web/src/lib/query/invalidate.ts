import type { QueryClient } from "@tanstack/react-query";
import { ApiProblem } from "@/api/problem";
import { OPERATION_ROLES, type OperationId } from "@/api/roles.generated";
import { adminKeys, compareKeys, docKeys, fileKeys, findingKeys, meKeys, prKeys, proposalKeys, swimlaneKeys, type QueryKey } from "./keys";
import { invalidateTargets, type Target } from "./targets";

// What a write invalidates, declared per operation, after the write answers. The live events say what other people did;
// this table says what THIS user's write changed, so the screen is right as soon as the write returns instead of when the
// event arrives (the event then finds the entries already fresh and refetches nothing that is not needed).
//
// The table is typed by the generated operation list: an operation that is not a GET and has no row is a compile error, and
// invalidate.test.ts proves the same at run time against the same list. An operation that changes nothing the app shows says
// so, with a reason (`none`), instead of leaving a gap that looks like an oversight.
//
// The rules the rows follow:
//  - they name what the write changed and nothing more: a file write touches that file's head, history, the trees above it,
//    the compares and grids of its swimlane, and the swimlane's summary (its last change), never the whole cache;
//  - a write that was SENT FOR APPROVAL (HTTP 202, `outcome: "proposal_created"`) changed nothing yet: only the queue;
//  - keys are built from the factories of keys.ts and nothing else, and every call goes through `invalidateTargets`, which
//    always filters and never touches an immutable entry;
//  - a secret-bearing operation (SECRET_OPERATIONS) invalidates only fixed keys of lists that hold no secret. The context of
//    a write has no field a secret could be put in, so no key and no filter can ever contain one.

type Outcome = { readonly outcome?: "applied" | "proposal_created" };
/** Nothing to say: any object with no field of its own (the index signature of `never` refuses a field that was not declared). */
type Nothing = Readonly<Record<string, never>>;

/** What an operation needs to say what it changed. An operation not listed here needs nothing. */
type SpecificContexts = {
  editFile: { readonly swimlane: string; readonly path: string } & Outcome;
  uploadFile: { readonly swimlane: string; readonly path: string } & Outcome;
  revertFile: { readonly swimlane: string; readonly path: string } & Outcome;
  deleteFile: { readonly swimlane: string; readonly path: string } & Outcome;
  applyDraft: { readonly swimlane: string; readonly path: string } & Outcome;
  approveProposal: { readonly id: number | string; readonly swimlane?: string; readonly path?: string } & Outcome;
  rejectProposal: { readonly id: number | string };
  restartService: { readonly swimlane: string } & Outcome;
  notifyServices: { readonly swimlane: string } & Outcome;
  createPr: Outcome;
  updateDoc: { readonly id: number | string };
  deleteDoc: { readonly id: number | string };
  adminUpdateUser: { readonly self?: boolean };
  adminPutAutoNotify: { readonly swimlane: string };
  adminPutTenants: { readonly swimlane: string };
  adminPutAgent: { readonly swimlane: string };
  adminDeleteAgent: { readonly swimlane: string };
};

export type MutatingOperationId = { [K in OperationId]: (typeof OPERATION_ROLES)[K]["method"] extends "GET" ? never : K }[OperationId];

export type WriteContext<K extends MutatingOperationId> = K extends keyof SpecificContexts ? SpecificContexts[K] : Nothing;

type Row<K extends MutatingOperationId> = { readonly targets: (ctx: WriteContext<K>) => readonly Target[] } | { readonly none: string };

const key = (k: QueryKey): Target => ({ kind: "key", key: k });
const sent = (ctx: Outcome): boolean => ctx.outcome === "proposal_created";

/** A file of a swimlane changed (by this write, or by the proposal it became). */
function fileChanged(swimlane: string, path: string): Target[] {
  return [
    key(fileKeys.head(swimlane, path)),
    { kind: "trees", swimlane, paths: [path] },
    key(fileKeys.history(swimlane, path)),
    { kind: "compares", swimlane },
    { kind: "grids", swimlane },
    key(swimlaneKeys.detail(swimlane)),
    key(swimlaneKeys.all()),
  ];
}

/** A write to a file: applied, or only sent for approval. */
const fileWrite = (ctx: { readonly swimlane: string; readonly path: string } & Outcome): Target[] => (sent(ctx) ? [key(proposalKeys.lists())] : fileChanged(ctx.swimlane, ctx.path));

/** A write that changes a swimlane's services: applied, or only sent for approval. */
const serviceWrite = (ctx: { readonly swimlane: string } & Outcome): Target[] =>
  sent(ctx) ? [key(proposalKeys.lists())] : [key(swimlaneKeys.pickup(ctx.swimlane)), key(swimlaneKeys.detail(ctx.swimlane)), key(swimlaneKeys.all())];

const section = (s: Parameters<typeof adminKeys.section>[0]) => ({ targets: () => [key(adminKeys.section(s))] });

export const WRITE_INVALIDATION: { readonly [K in MutatingOperationId]: Row<K> } = {
  // Access requests and users.
  adminApproveAccessRequest: { targets: () => [key(adminKeys.section("access-requests")), key(adminKeys.section("users"))] },
  adminRejectAccessRequest: section("access-requests"),
  adminUpdateUser: { targets: (ctx) => [key(adminKeys.section("users")), ...(ctx.self === true ? [key(meKeys.all())] : [])] },

  // Agents and the swimlanes they report for. The join token is a one-time secret: only the list of agents is refetched.
  adminPutAgent: { targets: (ctx) => [key(adminKeys.section("agents")), key(swimlaneKeys.all()), key(swimlaneKeys.detail(ctx.swimlane))] },
  adminDeleteAgent: { targets: (ctx) => [key(adminKeys.section("agents")), key(swimlaneKeys.all()), key(swimlaneKeys.detail(ctx.swimlane))] },
  adminIssueJoinToken: section("agents"),

  // Per-swimlane settings.
  adminPutAutoNotify: { targets: (ctx) => [key(adminKeys.section("auto-notify")), key(swimlaneKeys.detail(ctx.swimlane))] },
  adminPutTenants: { targets: (ctx) => [key(adminKeys.section("tenants")), key(swimlaneKeys.detail(ctx.swimlane)), key(swimlaneKeys.all()), key(compareKeys.grids())] },

  // Admin lists. Host tiers show on every swimlane; severity rules decide the severity of findings.
  adminPutHostTiers: { targets: () => [key(adminKeys.section("host-tiers")), key(swimlaneKeys.all()), key(swimlaneKeys.details())] },
  adminDeleteHostTiers: { targets: () => [key(adminKeys.section("host-tiers")), key(swimlaneKeys.all()), key(swimlaneKeys.details())] },
  adminPutSeverityRules: { targets: () => [key(adminKeys.section("severity-rules")), key(findingKeys.all())] },
  adminDeleteSeverityRules: { targets: () => [key(adminKeys.section("severity-rules")), key(findingKeys.all())] },
  adminPutFolderMappings: section("folder-mappings"),
  adminDeleteFolderMappings: section("folder-mappings"),
  adminPutMergeModes: section("merge-modes"),
  adminDeleteMergeModes: section("merge-modes"),
  adminPutNotification: section("notifications"),
  adminDeleteNotification: section("notifications"),
  adminPutOsLoginUser: section("os-login-users"),
  adminDeleteOsLoginUser: section("os-login-users"),
  adminPutPathMappings: section("path-mappings"),
  adminDeletePathMappings: section("path-mappings"),
  adminPutRetention: section("retention"),

  // Files.
  editFile: { targets: fileWrite },
  uploadFile: { targets: fileWrite },
  revertFile: { targets: fileWrite },
  deleteFile: { targets: fileWrite },
  applyDraft: { targets: (ctx) => [key(proposalKeys.lists()), ...fileWrite(ctx)] },
  createDraft: { targets: () => [key(proposalKeys.lists())] },

  // Proposals.
  approveProposal: {
    targets: (ctx) => {
      const decided = [key(proposalKeys.lists()), key(proposalKeys.detail(ctx.id))];
      if (sent(ctx)) return decided;
      return ctx.swimlane !== undefined && ctx.path !== undefined ? [...decided, ...fileChanged(ctx.swimlane, ctx.path)] : [...decided, key(swimlaneKeys.all())];
    },
  },
  rejectProposal: { targets: (ctx) => [key(proposalKeys.lists()), key(proposalKeys.detail(ctx.id))] },

  // Services.
  restartService: { targets: serviceWrite },
  notifyServices: { targets: serviceWrite },

  // Pull requests.
  createPr: { targets: (ctx) => [key(prKeys.lists()), ...(sent(ctx) ? [key(proposalKeys.lists())] : [])] },
  previewPr: { none: "A preview: it computes what a pull request would contain and writes nothing, so there is no cache to refresh." },
  previewFileEffect: { none: "A preview: it computes what a change would affect and writes nothing, so there is no cache to refresh." },

  // Documents.
  createDoc: { targets: () => [key(docKeys.lists()), key(docKeys.featureStatuses())] },
  updateDoc: { targets: (ctx) => [key(docKeys.lists()), key(docKeys.detail(ctx.id)), key(docKeys.featureStatuses())] },
  deleteDoc: { targets: (ctx) => [key(docKeys.lists()), key(docKeys.detail(ctx.id)), key(docKeys.featureStatuses())] },

  // My account.
  createAccessRequest: { targets: () => [key(meKeys.all())] },
  putGithubToken: { targets: () => [key(meKeys.githubCredential()), key(meKeys.all())] },
  deleteGithubToken: { targets: () => [key(meKeys.githubCredential()), key(meKeys.all())] },
  createMcpToken: { targets: () => [key(meKeys.mcpTokens())] },
  revokeMcpToken: { targets: () => [key(meKeys.mcpTokens())] },

  // Nothing to refresh.
  revealValue: { none: "A reveal reads one masked value and changes nothing the app shows; its answer lives only in component state and is never cached (S14b)." },
  authLogout: { none: "Signing out clears every cache and goes to the sign-in screen (src/auth); refetching here would ask for the previous user's data as nobody." },
  githubWebhook: { none: "GitHub calls this endpoint on the hub; the app never does, and nothing it shows is changed by the call from here." },
};

export const MUTATING_OPERATIONS = Object.keys(WRITE_INVALIDATION) as MutatingOperationId[];

/**
 * The operations that send a secret or return one: a token to store, a token that is shown once, a join token, a webhook URL,
 * a revealed value. They are never retried and their answers are never cached by the fetch layer (src/api/client.ts), they
 * go through `useSecretMutation` (gcTime 0, reset in a `finally`), and they never take part in an optimistic update.
 */
export const SECRET_OPERATIONS = ["putGithubToken", "createMcpToken", "adminIssueJoinToken", "adminPutNotification", "revealValue"] as const satisfies readonly MutatingOperationId[];

/** The targets of a write, or none when the operation declares that it changes nothing the app shows. */
export function writeTargets<K extends MutatingOperationId>(operation: K, context: WriteContext<K>): readonly Target[] {
  const row: Row<K> = WRITE_INVALIDATION[operation];
  return "targets" in row ? row.targets(context) : [];
}

/** Invalidate what the write changed. Resolves when the entries on screen have refetched. */
export function invalidateAfterWrite<K extends MutatingOperationId>(client: QueryClient, operation: K, context: WriteContext<K>): Promise<void> {
  return invalidateTargets(client, writeTargets(operation, context));
}

/**
 * Whether a FAILED write leaves the cache out of date. A conflict, a lock, a failed precondition and a server error mean the
 * thing is not as the screen shows it (or not as the user thinks); no answer at all means the write may have happened. A
 * refusal (400, 401, 403, 404, 413, 422, 429) changed nothing, and a cancelled request never left.
 */
export function shouldRefetchAfterError(error: unknown): boolean {
  if (!(error instanceof ApiProblem)) return false;
  if (error.status === 0) return error.kind === "network" || error.kind === "timeout";
  return error.status === 409 || error.status === 412 || error.status === 423 || error.status >= 500;
}
