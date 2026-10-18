import { FindingKind, Severity, type Finding, type LineRange } from "@/api/generated/model";
import { codeOf as kindCode } from "@/features/findings/kinds";

// How the Findings tab orders and words what the server found. Display only: the severity and the lines are the server's.

const RANK: Record<Severity, number> = { [Severity.critical]: 0, [Severity.high]: 1, [Severity.medium]: 2, [Severity.low]: 3 };
const KIND_RANK = Object.values(FindingKind);

/** Most severe first, then the check (C1 before C2), then the file. A finding with no file comes after those that have one. */
export function byUrgency(findings: readonly Finding[]): Finding[] {
  return [...findings].sort(
    (a, b) =>
      RANK[a.severity] - RANK[b.severity] ||
      KIND_RANK.indexOf(a.kind) - KIND_RANK.indexOf(b.kind) ||
      Number(a.path === null) - Number(b.path === null) ||
      (a.path ?? "").localeCompare(b.path ?? "") ||
      a.id - b.id,
  );
}

export const lineLabel = (lines: LineRange): string => (lines.start === lines.end ? `L${lines.start}` : `L${lines.start}–${lines.end}`);

export const codeOf = (finding: Finding): string => kindCode(finding.kind);
