import type { ObservationSource } from "@/api/generated/model";

// How a version of a file came to be seen, in the words of the history panel. Who changed it, and how sure the server is, are said
// by the shared attribution chip (src/components/attribution), which every screen uses.

const OBSERVED: Record<ObservationSource, string> = { scan: "Scan found a change", tool_write: "Tool write", sync: "Sync from Git" };

/** How a version came to be seen: written through the tool, synced from Git, or found by a scan. */
export function sourceLabel(source: ObservationSource): string {
  return OBSERVED[source];
}
