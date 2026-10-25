import type { AgentStatus, FileClass } from "@/api/generated/model";
import type { GateResult } from "@/components/app/write-gate";

// Who may press which button of the file view, and why not. `base` is the shell's answer for an Editor's write (the role, and
// whether the live stream is up); the rest is about this file and this swimlane. The first reason that applies is the one
// shown: the permission before everything, because waiting does not fix it.

export type GateInput = {
  /** `useWriteGate("editor")`. */
  base: GateResult;
  /** The swimlane's agent, or undefined while the swimlane is loading. Writes to NFS go through it. */
  agent: AgentStatus | undefined;
  lane: string;
  /** The text shown holds markers: it is a view of the file, not the file. */
  masked: boolean;
  fileClass: FileClass | undefined;
  withheld: boolean;
  /** A `token_expired` problem was seen: Git-dependent actions stay off until a new token is saved (W32). */
  tokenExpired: boolean;
  needsGithubToken: boolean;
};

export type FileGates = { edit: GateResult; upload: GateResult; delete: GateResult; revert: GateResult; pr: GateResult };

const no = (reason: string): GateResult => ({ allowed: false, reason });
const yes: GateResult = { allowed: true, reason: null };

export function fileGates(input: GateInput): FileGates {
  const { base, agent, masked, fileClass, withheld, tokenExpired, needsGithubToken } = input;
  if (!base.allowed) return { edit: base, upload: base, delete: base, revert: base, pr: base };
  if (withheld) {
    const reason = "The content of this file is withheld. There is no edit, upload or download.";
    return { edit: no(reason), upload: no(reason), delete: no(reason), revert: no(reason), pr: no(reason) };
  }
  // A write goes to the swimlane's NFS through its agent: until the agent is known to be there, it is off.
  const toNfs = agent === "connected" ? yes : no("Unavailable while the agent is offline");
  const pr = tokenExpired ? no("Your GitHub token expired. Update it in My GitHub token.") : needsGithubToken ? no("Connect your GitHub token first. A pull request is raised with your own token.") : yes;
  const edit = !toNfs.allowed
    ? toNfs
    : fileClass === "binary"
      ? no("A binary file cannot be edited here. Upload a new version instead.")
      : masked
        ? no("This view has masked values, so it is not the file and cannot be edited here.")
        : yes;
  return { edit, upload: toNfs, delete: toNfs, revert: toNfs, pr };
}
