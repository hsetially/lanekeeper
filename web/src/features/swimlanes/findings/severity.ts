import { FindingKind, type Finding, type LineRange } from "@/api/generated/model";
import { compareSeverity } from "@/components/attribution/severity";
import { codeOf as kindCode } from "@/features/findings/kinds";

// How the Findings tab orders and words what the server found. Display only: the severity and the lines are the server's.

const KIND_RANK = Object.values(FindingKind);

/** Most severe first, then the check (C1 before C2), then the file. A finding with no file comes after those that have one. */
export function byUrgency(findings: readonly Finding[]): Finding[] {
  return [...findings].sort(
    (a, b) =>
      compareSeverity(a.severity, b.severity) ||
      KIND_RANK.indexOf(a.kind) - KIND_RANK.indexOf(b.kind) ||
      Number(a.path === null) - Number(b.path === null) ||
      (a.path ?? "").localeCompare(b.path ?? "") ||
      a.id - b.id,
  );
}

export const lineLabel = (lines: LineRange): string => (lines.start === lines.end ? `L${lines.start}` : `L${lines.start}–${lines.end}`);

export const codeOf = (finding: Finding): string => kindCode(finding.kind);
