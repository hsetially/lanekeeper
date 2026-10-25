import { AttributionSource, Confidence, type Attribution } from "@/api/generated/model";
import { formatTime } from "@/lib/time";

// Who changed a file, how Lanekeeper knows, and how sure it is (D73), as words and as the steps of a bar. PURE: it reads the
// attribution the hub sent and nothing else, so it never decides anything about a change. The rules that keep it honest:
//
//  - a person is never named by this module. A name is what the caller's lookup says (the Admin screens have the user list; a
//    Viewer's screens do not), and without one the person is "Lanekeeper user" and the start of their id. The contract
//    carries a user as an id and nothing more (`Actor.user`), so any name here would be invented.
//  - the confidence is the server's. The one thing done to it: for the source `unknown` the contract carries `low` (the domain's
//    placeholder), where D73 says none, so it is shown as no score and not as a weak one.
//  - where the server names nobody, the words say so ("Unknown author"), except where the SOURCE is a finding of its own: an NFS
//    client was seen writing while no sync job ran (D73), and a sync job ran. Those are said as what they are.
//
// Text from the attribution (an OS login, a job name, an executable) comes from the NFS VM or a cluster and is untrusted: the
// strings returned here carry it as it came, and a screen draws every one through SafeText.

/** The levels the contract has, weakest first. */
export const CONFIDENCE_LEVELS: readonly Confidence[] = [Confidence.low, Confidence.medium, Confidence.high, Confidence.certain];

const CONFIDENCE_LABEL: Readonly<Record<Confidence, string>> = {
  [Confidence.low]: "Low",
  [Confidence.medium]: "Medium",
  [Confidence.high]: "High",
  [Confidence.certain]: "Certain",
};

// The bar has a step for each level and four in all: a quarter for low, the whole bar for certain.
const STEP: Readonly<Record<Confidence, 1 | 2 | 3 | 4>> = {
  [Confidence.low]: 1,
  [Confidence.medium]: 2,
  [Confidence.high]: 3,
  [Confidence.certain]: 4,
};

export type ConfidenceView = {
  /** The server's level, or null when there is no score. */
  readonly level: Confidence | null;
  readonly label: string;
  /** How many of the `steps` are filled: 1 to 4, or 0 for no score. */
  readonly step: 0 | 1 | 2 | 3 | 4;
  readonly steps: 4;
};

export function confidenceView(level: Confidence | null): ConfidenceView {
  if (level === null) return { level: null, label: "Unknown", step: 0, steps: 4 };
  return { level, label: CONFIDENCE_LABEL[level], step: STEP[level], steps: 4 };
}

/** The level to show for an attribution: the server's, or null for the source `unknown` (D73: none). */
export function shownConfidence(attribution: Pick<Attribution, "source" | "confidence">): Confidence | null {
  return attribution.source === AttributionSource.unknown ? null : attribution.confidence;
}

/** The name of a source as a column or a row label says it. */
export const SOURCE_LABEL: Readonly<Record<AttributionSource, string>> = {
  [AttributionSource.tool_write]: "Via Lanekeeper",
  [AttributionSource.sentinel_login]: "Edited on the NFS VM",
  [AttributionSource.sync_job]: "Sync job",
  [AttributionSource.nfs_client]: "NFS client",
  [AttributionSource.fs_owner_hint]: "File owner hint",
  [AttributionSource.unknown]: "Unknown",
};

// What follows the name in a line: "Marco Diaz via Lanekeeper", "dana.okoye edited on the NFS VM", "NFS client (no sync running)".
const HOW: Readonly<Record<AttributionSource, string>> = {
  [AttributionSource.tool_write]: "via Lanekeeper",
  [AttributionSource.sentinel_login]: "edited on the NFS VM",
  // A job is named by its name ("Sync job csp-tenant-data-sit7"); nothing follows it.
  [AttributionSource.sync_job]: "",
  [AttributionSource.nfs_client]: "(no sync running)",
  [AttributionSource.fs_owner_hint]: "(the file's owner, a hint)",
  [AttributionSource.unknown]: "",
};

// The design sets a middle dot between the name and these two ("Marco Diaz · via Lanekeeper"), and none before a bracketed note.
const DOTTED: ReadonlySet<AttributionSource> = new Set([AttributionSource.tool_write, AttributionSource.sentinel_login]);

/** What a source means, from D73's table: for the evidence panel. */
export const SOURCE_MEANING: Readonly<Record<AttributionSource, string>> = {
  [AttributionSource.tool_write]: "Written through Lanekeeper, so who made the change is recorded.",
  [AttributionSource.sentinel_login]: "The sentinel on the NFS VM saw a login write the file.",
  [AttributionSource.sync_job]: "A sync job was running when the file changed.",
  [AttributionSource.nfs_client]: "A client wrote the file over NFS while no sync job was running. Nobody is named.",
  [AttributionSource.fs_owner_hint]: "Only the owner of the file is known. That is a hint, not a record of who wrote it.",
  [AttributionSource.unknown]: "Nothing identified who changed the file.",
};

/** True for every attribution: it is upgraded when stronger evidence turns up, and never downgraded (D73). */
export const UPGRADE_NOTE = "Attribution is upgraded when stronger evidence turns up, and never downgraded.";

/** A name for a user id, if the caller has one. */
export type NameLookup = (userId: string) => string | undefined;

export type WhoContext = {
  /** The signed-in person's id: their own changes are "You". */
  readonly you?: string | null | undefined;
  readonly nameOf?: NameLookup | undefined;
};

/** The first eight characters of the object part of a user id (`<Entra tenant>:<object id>`): enough to tell two people apart. */
export function shortUserId(id: string): string {
  const object = id.includes(":") ? id.slice(id.indexOf(":") + 1) : id;
  return Array.from(object).slice(0, 8).join("");
}

const isYou = (id: string, context: WhoContext): boolean => context.you !== undefined && context.you !== null && context.you === id;

/** What a user is called: "You", the name the lookup has, else "Lanekeeper user" and the short id. Never a name made up. */
export function userLabel(id: string, context: WhoContext = {}): string {
  if (isYou(id, context)) return "You";
  const name = context.nameOf?.(id)?.trim();
  return name === undefined || name === "" ? `Lanekeeper user ${shortUserId(id)}` : name;
}

export type WhoKind = "you" | "user" | "os_login" | "job" | "client" | "sync" | "unknown";

export type AttributionWho = {
  readonly kind: WhoKind;
  /** The words for the screen. They can hold untrusted text (a name, a login, a job name): draw them with SafeText. */
  readonly label: string;
};

function whoOf(attribution: Attribution, context: WhoContext): AttributionWho {
  const { actor, source } = attribution;
  if (actor !== null) {
    switch (actor.kind) {
      case "user":
        return { kind: isYou(actor.user, context) ? "you" : "user", label: userLabel(actor.user, context) };
      case "os_login":
        return { kind: "os_login", label: actor.login };
      case "job":
        return { kind: "job", label: `Sync job ${actor.job.name}` };
    }
  }
  if (source === AttributionSource.nfs_client) return { kind: "client", label: "NFS client" };
  if (source === AttributionSource.sync_job) return { kind: "sync", label: "Sync job" };
  return { kind: "unknown", label: "Unknown author" };
}

export type AttributionView = {
  readonly who: AttributionWho;
  /** What follows the name; "" when nothing does. */
  readonly how: string;
  /** The design puts a middle dot between the name and `how` (not before a bracketed note). */
  readonly dotted: boolean;
  readonly source: AttributionSource;
  readonly sourceLabel: string;
  readonly confidence: ConfidenceView;
  /** False when the server has no evidence at all (source unknown, no score). */
  readonly known: boolean;
};

export function describeAttribution(attribution: Attribution, context: WhoContext = {}): AttributionView {
  const shown = shownConfidence(attribution);
  return {
    who: whoOf(attribution, context),
    how: HOW[attribution.source],
    dotted: DOTTED.has(attribution.source),
    source: attribution.source,
    sourceLabel: SOURCE_LABEL[attribution.source],
    confidence: confidenceView(shown),
    known: shown !== null,
  };
}

export type EvidenceRow = { readonly label: string; readonly value: string; readonly mono?: true };

/**
 * The facts of an attribution as label and value rows: the source, the confidence, who, and each piece of evidence the server
 * recorded (the executable, the login identity, when it was seen). A row that was not recorded is not there.
 */
export function evidenceRows(attribution: Attribution, context: WhoContext = {}): EvidenceRow[] {
  const rows: EvidenceRow[] = [
    { label: "Source", value: SOURCE_LABEL[attribution.source] },
    { label: "Confidence", value: shownConfidence(attribution) === null ? "None" : confidenceView(attribution.confidence).label },
  ];
  const { actor, evidence } = attribution;
  if (actor === null) {
    rows.push({ label: "Author", value: "Not identified" });
  } else {
    switch (actor.kind) {
      case "user":
        rows.push({ label: "Author", value: userLabel(actor.user, context) });
        break;
      case "os_login":
        rows.push({ label: "OS Login user", value: actor.login, mono: true });
        break;
      case "job":
        rows.push({ label: "Sync job", value: actor.job.name }, { label: "Job uid", value: actor.job.uid, mono: true });
        break;
    }
  }
  if (evidence.executable !== undefined && evidence.executable !== null) rows.push({ label: "Executable", value: evidence.executable, mono: true });
  if (evidence.login_identity !== undefined && evidence.login_identity !== null) rows.push({ label: "Login identity", value: evidence.login_identity, mono: true });
  if (evidence.observed_at !== undefined && evidence.observed_at !== null) rows.push({ label: "Observed", value: formatTime(evidence.observed_at, { seconds: true }) });
  return rows;
}
