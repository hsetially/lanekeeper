import type { Finding, LineRange } from "@/api/generated/model";
import { linkTo } from "@/routes/links";

/** `L41`, or `L209–217` for a range: the way the design writes the lines a finding names. */
export const lineLabel = (lines: LineRange): string => (lines.start === lines.end ? `L${lines.start}` : `L${lines.start}–${lines.end}`);

/** The address of the file a finding is about, at the first line it names: null for a finding that is about the swimlane, not a file. */
export function fileHref(finding: Pick<Finding, "swimlane" | "path" | "locations">): string | null {
  if (finding.path === null) return null;
  const first = finding.locations[0];
  return linkTo("files", { id: finding.swimlane }, { path: finding.path, ...(first === undefined ? {} : { line: String(first.lines.start) }) });
}
