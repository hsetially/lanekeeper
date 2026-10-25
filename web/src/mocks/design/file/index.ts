// The design-parity scenarios of slice S3 (the file view, diff, edit, history, dialogs, pull requests, docs panel and the draft
// page). Each is the File page of the handoff (`Lanekeeper File.dc.html`, and the States and Consequence dialogs it shares) in one
// state, and is named for it. The loader (../index.ts) picks up what this file exports; the data is in ./data.ts and the builder
// in ./build.ts. Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { Scenario } from "../../scenario.ts";
import { binaryFile, coreFile, deniedFile, largeFile, lineEndingFile } from "./data.ts";
import { fileScenario } from "./build.ts";
import { draftScenarios } from "./draft.ts";

const core = coreFile({ masked: true });
// The same file without the masked value: it can be edited. (A file whose text holds a marker opens read-only; the design's
// edit frames show the file with its secret out of the way.)
const editable = coreFile({ masked: false });

export const scenarios: readonly Scenario[] = [
  fileScenario({ id: "design-file-core", description: "File, view: the core tenant file for sit7 as Priya Shah (Admin), NFS ahead, with a masked value, history and docs.", file: core }),
  fileScenario({ id: "design-file-viewer", description: "File, Viewer (States P03): read-only. The write buttons are disabled with the role that is needed.", role: "viewer", file: core }),
  fileScenario({ id: "design-file-lineending", description: "File, line endings only: the gateway file has CRLF where Git has LF, and the Diff tab says so.", file: lineEndingFile(), compare: "eol-only", docs: false, history: false }),
  fileScenario({ id: "design-file-large", description: "File, large (6,140 lines): opens read-only, the editor is enabled on request.", file: largeFile(), compare: "identical", docs: false, history: false }),
  fileScenario({
    id: "design-file-diff-large",
    description: "File, Diff tab of a file of 6,140 lines: the comparison is 6,000 lines of hunks, a change every ten lines, tagged as the hub tags it. An Editor. The P10 gate's diff, on the real screen.",
    role: "editor",
    file: largeFile(),
    compare: "large",
    docs: false,
    history: false,
  }),
  fileScenario({ id: "design-file-binary", description: "File, binary: a JPEG, compared by picture, with no editor.", file: binaryFile(), compare: "binary", docs: false, history: false }),
  fileScenario({ id: "design-file-denied", description: "File, denied: the content of the keystore is withheld and there are no write actions.", file: deniedFile(), docs: false, history: false }),
  fileScenario({ id: "design-file-agent-offline", description: "File, agent offline (States O03): the last known file is shown and saving is off.", file: core, agent: "disconnected" }),
  fileScenario({ id: "design-file-edit", description: "File, edit: the core file without a masked value, so it opens in the editor; a save is applied.", file: editable }),
  fileScenario({ id: "design-file-edit-conflict", description: "File, edit, 409: someone else changed the file; nothing was written.", file: editable, write: "conflict" }),
  fileScenario({ id: "design-file-edit-locked", description: "File, edit, 423: the swimlane is locked until it is adopted.", file: editable, write: "locked", baseline: "unconfirmed" }),
  fileScenario({ id: "design-file-edit-approval", description: "File, edit by someone whose writes need approval: the save is sent for approval, not applied.", file: editable, requiresApproval: true, write: "approval" }),
  fileScenario({ id: "design-file-edit-secret", description: "File, edit, 422: the content holds a secret, and the save is refused.", file: editable, write: "secret" }),
  fileScenario({ id: "design-file-diff", description: "File, Diff tab: Git against NFS, three settings changed.", file: core }),
  fileScenario({ id: "design-file-pr", description: "File, Raise PR: the tenant branch or base, then the job.", file: core }),
  fileScenario({ id: "design-file-pr-exists", description: "File, Raise PR, 409: a pull request is already open for this change.", file: core, pr: "exists" }),
  fileScenario({ id: "design-file-pr-token-expired", description: "File, Raise PR: the person's GitHub token has expired.", file: core, pr: "token-expired" }),
  fileScenario({ id: "design-file-pr-no-access", description: "File, Raise PR: the token cannot push to the tenant repository.", file: core, pr: "no-access" }),
  fileScenario({ id: "design-file-pr-failed", description: "File, Raise PR: the job fails.", file: core, pr: "failed" }),
  ...draftScenarios,
];
