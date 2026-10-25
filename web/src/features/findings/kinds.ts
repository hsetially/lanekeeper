import { FindingKind } from "@/api/generated/model";

// The twelve kinds of finding of the contract, which are the checks C1 to C12 (the contract's order is the checks' order): for each,
// its code, a name for a list, what it means in one plain sentence or two, and what to do next. This is WORDS. Whether a
// finding exists, its kind, its severity and where it is are the server's facts; nothing here decides or re-derives any of them,
// and nothing here says more about one finding than its kind does (no hash, no count, no date).
//
// The design drew eight checks. Two of them mean something else in the contract (the design's C3 "override has no base file" and
// C5 "file does not parse": the contract's C3 is uneven tenant files and its C5 is drift), and C9 to C12 have no frame in the
// handoff, so their words are written here from the checks' definitions (docs/domain-model.md, the config-semantics skill).

/** What else the drawer can open for a kind, besides the file at its first line. */
export type Opens = "diff" | "history";

export type KindInfo = {
  /** `C1` to `C12`. */
  code: string;
  /** A name for a list or a menu. */
  name: string;
  /** What it means: one sentence, or two. */
  meaning: string;
  /** What to do, in order. Only steps this app has a place for: no marking, no restoring. */
  steps: readonly string[];
  opens: readonly Opens[];
};

export const KIND_INFO: Readonly<Record<FindingKind, KindInfo>> = {
  [FindingKind.missed_base_changes]: {
    code: "C1",
    name: "Tenant copy is behind base",
    meaning: "Base has changes that this tenant's copy of the file does not. A tenant file wins whenever it exists, so the swimlane never receives them.",
    steps: [
      "Open the file and look at the lines the finding names.",
      "Copy what base added into the tenant file, or delete the tenant copy if the swimlane needs no override, so that base applies in full.",
      "Raise a PR. The save tells you which services need a notify or a restart before you confirm it.",
    ],
    opens: [],
  },
  [FindingKind.redundant_tenant_file]: {
    code: "C2",
    name: "Override identical to base",
    meaning: "This tenant file says the same as the base file, so it changes nothing today. Because a tenant file wins whenever it exists, it hides every later base change from this swimlane.",
    steps: ["Delete the tenant copy so that the swimlane follows base, and raise a PR on the tenant repository.", "If the copy is there on purpose, leave it and plan to review it when base changes."],
    opens: [],
  },
  [FindingKind.uneven_tenant_files]: {
    code: "C3",
    name: "Tenant files are uneven",
    meaning: "Some tenants of this swimlane have their own copy of the file and others do not, so the tenants read different values.",
    steps: ["Decide whether every tenant of the swimlane should have its own copy.", "Add the missing copies or delete the extra ones, and raise a PR."],
    opens: [],
  },
  [FindingKind.orphan_file]: {
    code: "C4",
    name: "File not in the swimlane's config",
    meaning: "The file is not part of this swimlane's configuration: its suffix names a tenant the swimlane does not have, or nothing tracks it in Git. A rebuild or a sync would lose it.",
    steps: ["Open the file and check whether it is still needed.", "If it is, raise a PR to add it to the tenant repository, or rename it for a tenant the swimlane has. If not, delete it from the swimlane."],
    opens: [],
  },
  [FindingKind.drift]: {
    code: "C5",
    name: "NFS and Git disagree",
    meaning: "NFS, Git and the baseline do not agree on this file, so what the services read is not what Git says.",
    steps: ["Open the diff to see what differs.", "Decide which version is right. If NFS is, raise a PR so that Git has it. If Git is, replace the NFS file with it.", "A file changed on both sides needs the two versions put together by hand."],
    opens: ["diff"],
  },
  [FindingKind.environment_host_mismatch]: {
    code: "C6",
    name: "Points at another environment",
    meaning: "A host in this file belongs to a different environment than the swimlane, for example a dev host in a pre-production swimlane.",
    steps: ["Open the file at the line the finding names.", "Change the host to the one for this environment. If the file should point there on purpose, leave it as it is."],
    opens: [],
  },
  [FindingKind.overwritten_nfs_change]: {
    code: "C7",
    name: "Sync overwrote an NFS change",
    meaning: "Someone changed this file on the server, then a sync from Git replaced it with an older version. The change is gone from the file, but Lanekeeper still has it in the file's history.",
    steps: [
      "Open the file's history and find the version from before the sync.",
      "Save that version again so that the change is back on the swimlane.",
      "Raise a PR so that Git has the change, or the next sync overwrites it again.",
    ],
    opens: ["history"],
  },
  [FindingKind.duplicate_keys]: {
    code: "C8",
    name: "Duplicate keys",
    meaning: "The same key appears more than once in this file. Most YAML parsers keep the last one and silently ignore the others, so the value you read first may not be the one in effect.",
    steps: ["Open the file at each line the finding names.", "Decide which value is right and remove the others."],
    opens: [],
  },
  [FindingKind.ambiguous_file_name]: {
    code: "C9",
    name: "Ambiguous file name",
    meaning: "The same file name is in more than one folder that is not a channel folder. The config-server treats file names as one namespace, so which copy a service gets is not clear from the name.",
    steps: ["Decide which folder should hold the file.", "Rename or delete the others, and raise a PR. Check which services ask for the file by name first."],
    opens: [],
  },
  [FindingKind.unresolved_placeholder]: {
    code: "C10",
    name: "Unresolved placeholder",
    meaning: "The file uses a placeholder that is neither a setting in the swimlane's property view nor the name of an environment variable of any service that reads the file, so it may not become a value.",
    steps: ["Open the file at the line the finding names and check the spelling of the placeholder.", "Add the setting or the environment variable it should refer to, or remove the placeholder."],
    opens: [],
  },
  [FindingKind.config_server_restart_required]: {
    code: "C11",
    name: "Config-server restart needed",
    meaning: "The config-server keeps the folders and channels it had when it started. A folder was created since, channels.yml changed, or a channel folder and the channel list disagree, so it serves a different set of files than NFS holds.",
    steps: ["Restart the config-server. This takes the Operator role.", "The finding clears once the config-server has restarted and reads the folders and channels as they are now."],
    opens: [],
  },
  [FindingKind.served_mismatch]: {
    code: "C12",
    name: "Served file differs",
    meaning: "The nightly check asked the config-server for this file and it served other content than Lanekeeper renders for it, or did not serve it at all.",
    steps: [
      "If the swimlane also has a config-server restart finding (C11), deal with that first: until the config-server restarts, what it serves and what is on NFS differ by design.",
      "Otherwise open the file and compare it with what the config-server serves, and find where the two come apart.",
    ],
    opens: [],
  },
};

/** The kinds in the order of the checks, C1 first. */
export const KINDS: readonly FindingKind[] = Object.values(FindingKind);

export const codeOf = (kind: FindingKind): string => KIND_INFO[kind].code;
export const nameOf = (kind: FindingKind): string => KIND_INFO[kind].name;

/** The kind of a code (`C8`, or `c8`), or undefined for anything that is not a check. */
export function kindOfCode(code: string): FindingKind | undefined {
  const wanted = code.toUpperCase();
  return KINDS.find((kind) => KIND_INFO[kind].code === wanted);
}
