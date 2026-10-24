import { ChangeKind, FindingKind, Severity, type SeverityMatch } from "@/api/generated/model";
import { KIND_INFO } from "@/features/findings/kinds";
import { hasHiddenOrBackslash } from "@/lib/text/control";

// The admin's severity rules, as words and as the checks a form makes before it sends. NOTHING HERE RATES ANYTHING: which rules match
// a change or a finding, and the severity that comes out, are the engine's and the registry's (rule 7, D103). What this file knows of
// them is what the admin has to be told, from the engine's own documentation of the rules:
//  - a rule matches when ALL the conditions it states hold (a list holds when ANY of its entries does), and a rule that states none
//    matches everything;
//  - the severity is the HIGHEST of the enabled rules that match, the built-in ones included, so a rule can raise a severity and can
//    never lower one or switch a built-in rule off, and the order of the rules (the contract's `priority`) does not matter;
//  - a glob matches the whole path from the root, case-sensitively, and `*` crosses `/`;
//  - the registry does not look at the individual setting changes of a file when it rates it, so a change type or a setting glob
//    has nothing to match today (D103, open point 1).

/** The contract needs a `priority` (0 to 10,000). The hub does not use it, so a new rule gets this one and an existing rule keeps its own. */
export const DEFAULT_PRIORITY = 100;

/** Most serious first, which is the order a person reads them in. */
export const SEVERITY_CHOICES: readonly Severity[] = [Severity.critical, Severity.high, Severity.medium, Severity.low];

export const severityLabel = (severity: Severity): string => severity.charAt(0).toUpperCase() + severity.slice(1);

/** The twelve checks, in the contract's order (which is the order of C1 to C12). */
export const FINDING_CHOICES: readonly FindingKind[] = Object.values(FindingKind);
export const CHANGE_CHOICES: readonly ChangeKind[] = Object.values(ChangeKind);

export const findingLabel = (kind: FindingKind): string => `${KIND_INFO[kind].code} ${KIND_INFO[kind].name}`;

const CHANGE_LABELS: Readonly<Record<ChangeKind, string>> = {
  [ChangeKind.added]: "Setting added",
  [ChangeKind.removed]: "Setting removed",
  [ChangeKind.changed]: "Setting value changed",
  [ChangeKind.type_changed]: "Setting changed type",
};
export const changeLabel = (kind: ChangeKind): string => CHANGE_LABELS[kind];

const MAX_GLOB_BYTES = 256;
const encoder = new TextEncoder();

const count = (text: string, char: string): number => text.split(char).length - 1;

/**
 * The message to show beside a glob, or null when it can be sent. An empty glob is a condition that is not stated. The hub counts
 * bytes (256), refuses a glob it cannot compile, and skips a rule it cannot read without saying which, so what can be known here is
 * said here. The message never repeats the glob.
 */
export function checkRuleGlob(value: string): string | null {
  const glob = value.trim();
  if (glob === "") return null;
  if (encoder.encode(glob).length > MAX_GLOB_BYTES) return `At most ${MAX_GLOB_BYTES} bytes`;
  if (hasHiddenOrBackslash(glob)) return "A path can't contain backslashes or control characters";
  if (glob.startsWith("/")) return "Paths start at the config root, with no leading /";
  if (count(glob, "[") > count(glob, "]")) return "Invalid glob: unclosed [";
  if (count(glob, "{") > count(glob, "}")) return "Invalid glob: unclosed {";
  return null;
}

export type MatchForm = {
  readonly findingKinds: readonly FindingKind[];
  readonly changeKinds: readonly ChangeKind[];
  readonly pathGlob: string;
  readonly settingGlob: string;
};

/** The contract's `SeverityMatch` for a form: only the conditions that are stated. */
export function buildMatch(form: MatchForm): SeverityMatch {
  const path = form.pathGlob.trim();
  const setting = form.settingGlob.trim();
  return {
    ...(form.findingKinds.length > 0 ? { finding_kinds: [...form.findingKinds] } : {}),
    ...(form.changeKinds.length > 0 ? { change_kinds: [...form.changeKinds] } : {}),
    ...(path === "" ? {} : { path_glob: path }),
    ...(setting === "" ? {} : { setting_glob: setting }),
  };
}

export type MatchPart = { readonly label: string; readonly value: string; readonly glob: boolean };

/** The conditions a rule states, for a list: "Findings C6, C5", "Path services/**". */
export function matchParts(match: SeverityMatch): MatchPart[] {
  const parts: MatchPart[] = [];
  const kinds = match.finding_kinds ?? [];
  if (kinds.length > 0) parts.push({ label: "Findings", value: kinds.map((k) => KIND_INFO[k].code).join(", "), glob: false });
  const changes = match.change_kinds ?? [];
  if (changes.length > 0) parts.push({ label: "Changes", value: changes.map(changeLabel).join(", "), glob: false });
  if (match.path_glob !== undefined && match.path_glob !== null && match.path_glob !== "") parts.push({ label: "Path", value: match.path_glob, glob: true });
  if (match.setting_glob !== undefined && match.setting_glob !== null && match.setting_glob !== "") parts.push({ label: "Setting", value: match.setting_glob, glob: true });
  return parts;
}

/** A rule that states no condition matches every change and every finding. */
export const matchesEverything = (match: SeverityMatch): boolean => matchParts(match).length === 0;

/** A change type or a setting glob has nothing to match when the registry rates a file or a finding (D103, open point 1). */
export function notRatedYet(match: SeverityMatch): boolean {
  return (match.change_kinds ?? []).length > 0 || (match.setting_glob !== undefined && match.setting_glob !== null && match.setting_glob !== "");
}
