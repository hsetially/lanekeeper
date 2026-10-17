import type { QueryClient } from "@tanstack/react-query";
import {
  adminApproveAccessRequest,
  adminDeleteAgent,
  adminDeleteFolderMappings,
  adminDeleteHostTiers,
  adminDeleteMergeModes,
  adminDeleteNotification,
  adminDeleteOsLoginUser,
  adminDeletePathMappings,
  adminDeleteSeverityRules,
  adminIssueJoinToken,
  adminPutAgent,
  adminPutAutoNotify,
  adminPutFolderMappings,
  adminPutHostTiers,
  adminPutMergeModes,
  adminPutNotification,
  adminPutOsLoginUser,
  adminPutPathMappings,
  adminPutRetention,
  adminPutSeverityRules,
  adminPutTenants,
  adminRejectAccessRequest,
  adminUpdateUser,
} from "@/api/generated/endpoints/admin/admin";
import type {
  AccessApproval,
  AccessRequest,
  AdminPutRetentionParams,
  AgentRegistration,
  AgentRegistrationInput,
  AutoNotify,
  FolderMapping,
  FolderMappingInput,
  HostTier,
  HostTierInput,
  MergeModeRule,
  MergeModeRuleInput,
  NotificationChannelInput,
  OsLoginUser,
  OsLoginUserInput,
  PathMappingRule,
  PathMappingRuleInput,
  RejectRequest,
  RetentionInput,
  RetentionSettings,
  SeverityRule,
  SeverityRuleInput,
  TenantSetInput,
  User,
  UserUpdate,
} from "@/api/generated/model";
import { invalidateAfterWrite } from "@/lib/query/invalidate";
import { optimisticWriteMutation } from "@/lib/query/optimistic";
import { writeMutation } from "@/lib/query/write";

// Admin writes. A PUT here replaces a whole rule or setting, and most can be refused (422), so they show the server's answer.
// The exception is the auto-notify switch (`setAutoNotify`), the one write of the app that is optimistic: see
// src/lib/query/optimistic.ts for why it, and only it, is safe to show before it answers.
//
// Two of them carry a secret (a webhook URL, a join token) and are plain requests for `useSecretMutation`, not mutation options.

type Put<I> = { id: string; body: I };
type PutLogin<I> = { login: string; body: I };

export const adminMutations = {
  approveAccessRequest: (client: QueryClient) =>
    writeMutation<"adminApproveAccessRequest", { id: number; body: AccessApproval }, User>(client, { operation: "adminApproveAccessRequest", run: ({ id, body }) => adminApproveAccessRequest(id, body), context: () => ({}) }),
  rejectAccessRequest: (client: QueryClient) =>
    writeMutation<"adminRejectAccessRequest", { id: number; body: RejectRequest }, AccessRequest>(client, { operation: "adminRejectAccessRequest", run: ({ id, body }) => adminRejectAccessRequest(id, body), context: () => ({}) }),
  /** `self` says the admin changed their own account, so the session's user is refetched too. */
  updateUser: (client: QueryClient) =>
    writeMutation<"adminUpdateUser", { id: string; body: UserUpdate; self?: boolean }, User>(client, { operation: "adminUpdateUser", run: ({ id, body }) => adminUpdateUser(id, body), context: ({ self }) => ({ ...(self === undefined ? {} : { self }) }) }),
  putAgent: (client: QueryClient) =>
    writeMutation<"adminPutAgent", { swimlane: string; body: AgentRegistrationInput }, AgentRegistration>(client, { operation: "adminPutAgent", run: ({ swimlane, body }) => adminPutAgent(swimlane, body), context: ({ swimlane }) => ({ swimlane }) }),
  deleteAgent: (client: QueryClient) => writeMutation<"adminDeleteAgent", { swimlane: string }, unknown>(client, { operation: "adminDeleteAgent", run: ({ swimlane }) => adminDeleteAgent(swimlane), context: ({ swimlane }) => ({ swimlane }) }),
  putTenants: (client: QueryClient) =>
    writeMutation<"adminPutTenants", { swimlane: string; body: TenantSetInput }, TenantSetInput>(client, { operation: "adminPutTenants", run: ({ swimlane, body }) => adminPutTenants(swimlane, body), context: ({ swimlane }) => ({ swimlane }) }),
  /** The auto-notify switch: optimistic, rolled back on failure, and refetched either way. */
  setAutoNotify: (client: QueryClient) =>
    optimisticWriteMutation<"adminPutAutoNotify", { swimlane: string; body: AutoNotify }, AutoNotify>(client, {
      operation: "adminPutAutoNotify",
      run: ({ swimlane, body }) => adminPutAutoNotify(swimlane, body),
      context: ({ swimlane }) => ({ swimlane }),
    }),
  putFolderMapping: (client: QueryClient) =>
    writeMutation<"adminPutFolderMappings", Put<FolderMappingInput>, FolderMapping>(client, { operation: "adminPutFolderMappings", run: ({ id, body }) => adminPutFolderMappings(id, body), context: () => ({}) }),
  deleteFolderMapping: (client: QueryClient) => writeMutation<"adminDeleteFolderMappings", string, unknown>(client, { operation: "adminDeleteFolderMappings", run: (id) => adminDeleteFolderMappings(id), context: () => ({}) }),
  putHostTier: (client: QueryClient) => writeMutation<"adminPutHostTiers", Put<HostTierInput>, HostTier>(client, { operation: "adminPutHostTiers", run: ({ id, body }) => adminPutHostTiers(id, body), context: () => ({}) }),
  deleteHostTier: (client: QueryClient) => writeMutation<"adminDeleteHostTiers", string, unknown>(client, { operation: "adminDeleteHostTiers", run: (id) => adminDeleteHostTiers(id), context: () => ({}) }),
  putMergeMode: (client: QueryClient) => writeMutation<"adminPutMergeModes", Put<MergeModeRuleInput>, MergeModeRule>(client, { operation: "adminPutMergeModes", run: ({ id, body }) => adminPutMergeModes(id, body), context: () => ({}) }),
  deleteMergeMode: (client: QueryClient) => writeMutation<"adminDeleteMergeModes", string, unknown>(client, { operation: "adminDeleteMergeModes", run: (id) => adminDeleteMergeModes(id), context: () => ({}) }),
  deleteNotification: (client: QueryClient) => writeMutation<"adminDeleteNotification", string, unknown>(client, { operation: "adminDeleteNotification", run: (id) => adminDeleteNotification(id), context: () => ({}) }),
  putOsLoginUser: (client: QueryClient) => writeMutation<"adminPutOsLoginUser", PutLogin<OsLoginUserInput>, OsLoginUser>(client, { operation: "adminPutOsLoginUser", run: ({ login, body }) => adminPutOsLoginUser(login, body), context: () => ({}) }),
  deleteOsLoginUser: (client: QueryClient) => writeMutation<"adminDeleteOsLoginUser", string, unknown>(client, { operation: "adminDeleteOsLoginUser", run: (login) => adminDeleteOsLoginUser(login), context: () => ({}) }),
  putPathMapping: (client: QueryClient) => writeMutation<"adminPutPathMappings", Put<PathMappingRuleInput>, PathMappingRule>(client, { operation: "adminPutPathMappings", run: ({ id, body }) => adminPutPathMappings(id, body), context: () => ({}) }),
  deletePathMapping: (client: QueryClient) => writeMutation<"adminDeletePathMappings", string, unknown>(client, { operation: "adminDeletePathMappings", run: (id) => adminDeletePathMappings(id), context: () => ({}) }),
  putSeverityRule: (client: QueryClient) => writeMutation<"adminPutSeverityRules", Put<SeverityRuleInput>, SeverityRule>(client, { operation: "adminPutSeverityRules", run: ({ id, body }) => adminPutSeverityRules(id, body), context: () => ({}) }),
  deleteSeverityRule: (client: QueryClient) => writeMutation<"adminDeleteSeverityRules", string, unknown>(client, { operation: "adminDeleteSeverityRules", run: (id) => adminDeleteSeverityRules(id), context: () => ({}) }),
  putRetention: (client: QueryClient) =>
    writeMutation<"adminPutRetention", { body: RetentionInput }, RetentionSettings>(client, { operation: "adminPutRetention", run: ({ body }) => adminPutRetention(body), context: () => ({}) }),
};

/** What a retention change would remove, without changing it (`dry_run`). It writes nothing, so nothing is refetched after it. */
export const previewRetentionRequest = (body: RetentionInput, params: AdminPutRetentionParams = { dry_run: true }) => adminPutRetention(body, params);

/** Set a notification channel (its webhook URL is a secret). For `useSecretMutation`: the URL stays in the caller's state. */
export const putNotificationRequest = (id: string, body: NotificationChannelInput) => adminPutNotification(id, body);
/** Issue a one-time join token for an agent. The answer holds the token once; it goes to the caller and nowhere else. */
export const issueJoinTokenRequest = (swimlane: string) => adminIssueJoinToken(swimlane);
/** What to refresh after one of the two secret requests above succeeded. Fixed keys: nothing of the secret. */
export const afterAdminSecretWrite = (client: QueryClient, operation: "adminPutNotification" | "adminIssueJoinToken"): Promise<void> => invalidateAfterWrite(client, operation, {});
