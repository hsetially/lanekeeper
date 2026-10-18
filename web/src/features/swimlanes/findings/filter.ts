import { FindingKind, Severity } from "@/api/generated/model";
import { codeOf } from "@/features/findings/kinds";
import { findingTitle } from "../format";

// The Findings tab can be narrowed from the address: `code` (C1 to C12), `kind` (the check's name) and `severity` (that and above).
// A value that is not one of the contract's is dropped, so a link with a typo shows everything rather than nothing.

export type FindingsFilter = { kinds: FindingKind[] | undefined; minSeverity: Severity | undefined; label: string | null };

const KINDS = Object.values(FindingKind);
const SEVERITIES = Object.values(Severity);

function kindOf(search: { readonly code?: string; readonly kind?: string }): FindingKind | undefined {
  if (search.kind !== undefined) {
    const found = KINDS.find((kind) => kind === search.kind);
    if (found !== undefined) return found;
  }
  if (search.code !== undefined) {
    const wanted = search.code.toUpperCase();
    return KINDS.find((kind) => codeOf(kind) === wanted);
  }
  return undefined;
}

export function codeFilter(search: { readonly code?: string; readonly kind?: string; readonly severity?: string }): FindingsFilter {
  const kind = kindOf(search);
  const minSeverity = SEVERITIES.find((s) => s === search.severity);
  const parts: string[] = [];
  if (kind !== undefined) parts.push(`${codeOf(kind)}, ${findingTitle(kind).toLowerCase()}`);
  if (minSeverity !== undefined) parts.push(`${minSeverity} severity and above`);
  return { kinds: kind === undefined ? undefined : [kind], minSeverity, label: parts.length === 0 ? null : parts.join(", ") };
}
