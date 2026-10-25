import { SentinelInfoStatus, type SentinelInfo } from "@/api/generated/model";

// How the Sentinels section words what the hub says about each sentinel. The STATUS is the hub's (connected, disconnected, never
// seen): this page never decides that a sentinel is silent from the age of its last-seen time, because the contract carries no limit
// to measure it against (the design's "alert after N minutes" setting has no operation), and because that time is the sentinel's last
// hello or batch of records, not a heartbeat: the hub does not record heartbeats, so a VM on which nothing is written can show an old
// time while its sentinel is connected (D123). A status it does not know is not "connected".

export type SentinelTone = "ok" | "gap" | "unknown";

export type SentinelView = {
  readonly label: string;
  readonly tone: SentinelTone;
  /** The sentinel has a connection to the hub right now. */
  readonly connected: boolean;
};

export function sentinelView(status: SentinelInfoStatus): SentinelView {
  switch (status) {
    case SentinelInfoStatus.connected:
      return { label: "Connected", tone: "ok", connected: true };
    case SentinelInfoStatus.disconnected:
      return { label: "Disconnected", tone: "gap", connected: false };
    case SentinelInfoStatus.never_seen:
      return { label: "Never seen", tone: "unknown", connected: false };
    default:
      return { label: "Not connected", tone: "gap", connected: false };
  }
}

/** The sentinels that were connected and are not: the ones whose VM's edits cannot be attributed right now. */
export function disconnected(rows: readonly SentinelInfo[]): SentinelInfo[] {
  return rows.filter((row) => row.status === SentinelInfoStatus.disconnected);
}

/** "6 registered · 1 disconnected · 1 never seen". */
export function countText(rows: readonly SentinelInfo[]): string {
  const gone = disconnected(rows).length;
  const never = rows.filter((row) => row.status === SentinelInfoStatus.never_seen).length;
  const parts = [`${rows.length} registered`];
  if (gone > 0) parts.push(`${gone} disconnected`);
  if (never > 0) parts.push(`${never} never seen`);
  return parts.join(" · ");
}

/** The records a sentinel holds on its VM that the hub has not received. */
export function backlogText(records: number): string {
  if (records <= 0) return "None";
  return `${records.toLocaleString("en-US")} ${records === 1 ? "record" : "records"}`;
}
