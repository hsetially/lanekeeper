import type { Attribution, AttributionSource, Confidence, ObservationSource } from "@/api/generated/model";

// Who changed a file and how sure the server is (D73), in the words of the history panel. The contract names a person by id
// only (`Actor.user`), never by name, so another person is "Another Lanekeeper user" here: a name needs a directory the
// contract does not have (design/DEVIATIONS.md). Attribution chips for the rest of the app are T5's.

export type AttributionView = {
  who: string;
  how: string;
  confidence: "Certain" | "High" | "Medium" | "Low" | "Unknown";
  /** Of four bars, how many are filled. */
  bars: 0 | 1 | 2 | 3 | 4;
};

const HOW: Record<AttributionSource, string> = {
  tool_write: "via Lanekeeper",
  sentinel_login: "at an NFS login",
  sync_job: "by a sync job",
  nfs_client: "from an NFS client",
  fs_owner_hint: "by the file owner (a hint)",
  unknown: "found by scan",
};

const LEVEL: Record<Confidence, { label: AttributionView["confidence"]; bars: AttributionView["bars"] }> = {
  certain: { label: "Certain", bars: 4 },
  high: { label: "High", bars: 3 },
  medium: { label: "Medium", bars: 2 },
  low: { label: "Low", bars: 1 },
};

export function describeAttribution(attribution: Attribution, meId: string | null): AttributionView {
  const { actor } = attribution;
  const how = HOW[attribution.source];
  if (actor === null) return { who: "Unknown", how, confidence: "Unknown", bars: 0 };
  const level = LEVEL[attribution.confidence];
  const who = actor.kind === "user" ? (actor.user === meId ? "You" : "Another Lanekeeper user") : actor.kind === "os_login" ? actor.login : `Sync job ${actor.job.name}`;
  return { who, how, confidence: level.label, bars: level.bars };
}

const OBSERVED: Record<ObservationSource, string> = { scan: "Scan found a change", tool_write: "Tool write", sync: "Sync from Git" };

/** How a version came to be seen: written through the tool, synced from Git, or found by a scan. */
export function sourceLabel(source: ObservationSource): string {
  return OBSERVED[source];
}
