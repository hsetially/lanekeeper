import { DriftState, FindingKind, type VersionLabel } from "@/api/generated/model";

// Small facts the swimlane screens print the same way: a base version, the drift states of a swimlane in order of urgency, the
// code and the words of a finding. Display only: what a state IS comes from the engine, never from here.

/** `v2.3.1 + 4` (four commits past the tag), `v2.3.1` at the tag, the short commit when there is no tag, a dash for none. */
export function versionLabel(version: VersionLabel | null | undefined): string {
  if (version === null || version === undefined) return "—";
  if (version.tag === null) return version.commit.slice(0, 7);
  return version.distance > 0 ? `${version.tag} + ${version.distance}` : version.tag;
}

// Most urgent first. In sync is the absence of a problem and is not a chip.
const CHIP_ORDER: readonly DriftState[] = [DriftState.conflict, DriftState.nfs_ahead, DriftState.git_ahead, DriftState.unknown, DriftState.untracked, DriftState.intentional_divergence];

export type DriftChip = { readonly state: DriftState; readonly count: number };

/** The chips of the Drift column: a state and how many files, for every state that needs a look. */
export function driftChips(counts: readonly (readonly [DriftState, number])[]): DriftChip[] {
  const by = new Map<DriftState, number>();
  for (const [state, count] of counts) {
    if (Number.isInteger(count) && count > 0) by.set(state, (by.get(state) ?? 0) + count);
  }
  return CHIP_ORDER.filter((state) => by.has(state)).map((state) => ({ state, count: by.get(state) ?? 0 }));
}

const FINDING_TITLE: Record<FindingKind, string> = {
  [FindingKind.missed_base_changes]: "Missed base changes",
  [FindingKind.redundant_tenant_file]: "Redundant tenant file",
  [FindingKind.uneven_tenant_files]: "Uneven tenant files",
  [FindingKind.orphan_file]: "Orphan file",
  [FindingKind.drift]: "Drift",
  [FindingKind.environment_host_mismatch]: "Environment and host mismatch",
  [FindingKind.overwritten_nfs_change]: "Overwritten NFS change",
  [FindingKind.duplicate_keys]: "Duplicate keys",
  [FindingKind.ambiguous_file_name]: "Ambiguous file name",
  [FindingKind.unresolved_placeholder]: "Unresolved placeholder",
  [FindingKind.config_server_restart_required]: "Config-server restart required",
  [FindingKind.served_mismatch]: "Served config differs",
};

export const findingTitle = (kind: FindingKind): string => FINDING_TITLE[kind];
