import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import type { FileEntry, SwimlaneDetail } from "@/api/generated/model";
import { compareFileQuery } from "@/features/compare/queries";
import { compareSides, sideLabel, type Side, type SideContext } from "./refs";

// The comparison of a file with another version of it, asked of the engine. What the sides are depends on the swimlane (its
// tenants and base version) and on the file (a tenant file's Git side is its tenant's branch); until both are known nothing is
// asked. `enabled` lets the page ask for the count on the Diff tab before the tab is opened.

export type ComparisonInput = {
  lane: string;
  path: string;
  entry: FileEntry | null;
  detail: SwimlaneDetail | undefined;
  side: Side;
  swapped: boolean;
  enabled: boolean;
};

export function useComparison({ lane, path, entry, detail, side, swapped, enabled }: ComparisonInput) {
  const context = useMemo<SideContext | null>(() => {
    if (detail === undefined || entry === null) return null;
    const version = detail.base_version;
    return {
      lane,
      tenant: entry.tenant ?? null,
      baseCommit: version?.commit ?? null,
      baseLabel: version === null ? "base" : version.distance > 0 ? `${version.tag ?? version.commit.slice(0, 7)} + ${version.distance}` : (version.tag ?? version.commit.slice(0, 7)),
    };
  }, [lane, detail, entry]);

  const sides = context === null ? null : compareSides(side, swapped, context);
  const file = entry?.logical_file ?? path;
  const query = useQuery({
    ...compareFileQuery(sides?.left ?? "nfs:unknown", sides?.right ?? "nfs:unknown", file, null),
    enabled: enabled && sides !== null,
  });
  const labels = context === null ? null : { chosen: sideLabel(side, context), nfs: "NFS (current)" };
  return { context, sides, query, labels, ready: context !== null };
}
