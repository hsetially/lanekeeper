// The data of the "As served" scenarios (T6 slice C): a resource file as the config-server answers for one application, tenant and
// channel, in the shapes the hub can send it (plain text, masked, withheld, not text) and the answers around it. Synthetic: nothing here
// is a real tenant's configuration, and the "secret" is a made-up marker. Loaded by Node (the e2e harness) as well as by Vite: relative
// imports with the `.ts` extension and `import type`.
import type { Mask, ServedFile } from "../../../api/generated/model/index.ts";

export const LANE = "sitb";
export const TENANTS = ["sit7", "sit9"] as const;
export const APPLICATION = "tx-infinity-core";
export const FILE = "tx-infinity-api/connection-pool.yml";
export const NOW = 1_760_000_000_000;
export const SERVED_PATH = "/api/v1/swimlanes/{id}/served";

export const H_SERVED = "5c1e0a97d3b8426f9a60e4d1c2b7a85f3e9d0c64b1a7f28e5d3c9b0a6e4f1d27";
export const H_DENIED = "9d2c47e1b05a8f36c4e2d7a190b3f5c8e6a2d4b7013c9e5f8a1d6b2c4e7f0a39";

const enc = new TextEncoder();

/** The marker the hub writes for a value the scan flagged (S14b): its length and the rule. */
export const MARKER = "[REDACTED: 22 chars, generic_secret]";

/** The lines of the file as the config-server serves it for `sit7`: `${POOL_SIZE}` was replaced, `${AUDIT_SINK}` had no value. */
export function servedLines(over: { password?: string } = {}): string[] {
  return [
    "# connection-pool.yml, as served to tx-infinity-core for sit7",
    "pool:",
    "  size: 24",
    "  timeoutMs: 30000",
    "  url: jdbc:postgresql://db.sit7.example.test:5432/core",
    "  user: core_app",
    `  password: ${over.password ?? MARKER}`,
    "  audit: ${AUDIT_SINK}",
    "",
  ];
}

export const servedText = (over: { password?: string } = {}): string => servedLines(over).join("\n");

/** The mask of the first `needle` in `text` on `line`, with the byte offsets the server counts (UTF-8, into the whole text, no `setting`). */
export function maskOf(text: string, line: number, needle: string): Mask {
  const start = enc.encode(text.slice(0, text.indexOf(needle))).length;
  return { line, start, end: start + enc.encode(needle).length };
}

/** A served file. `over` replaces any member; the defaults are the plain text above, with its one placeholder left. */
export function servedFile(over: Partial<ServedFile> = {}): ServedFile {
  return {
    swimlane: LANE,
    application: APPLICATION,
    tenant: "sit7",
    channel: null,
    file: FILE,
    role: "resource",
    hash: H_SERVED,
    content_withheld: false,
    text: servedText({ password: "********" }),
    sources: [{ path: "tx-infinity-api/connection-pool-sit7.yml", rank: 0 }],
    unresolved_placeholders: ["AUDIT_SINK"],
    ...over,
  };
}

/** The same file with the password masked, as the hub sends it once it masks (M1): the marker in the text and where it sits. */
export function maskedServedFile(over: Partial<ServedFile> = {}): ServedFile {
  const text = servedText();
  return servedFile({ text, masks: [maskOf(text, 7, MARKER)], ...over });
}

/** A file the hub never shows (a keystore): its hash and nothing of its content. */
export function withheldServedFile(over: Partial<ServedFile> = {}): ServedFile {
  return servedFile({ file: "security/keystore.jks", role: "resource", hash: H_DENIED, content_withheld: true, text: null, sources: [], unresolved_placeholders: [], ...over });
}

/** An answer that is not text (a binary file, or text the hub cannot decode): `text` is null and the file is not withheld. */
export function binaryServedFile(over: Partial<ServedFile> = {}): ServedFile {
  return servedFile({ file: "tx-infinity-api/logo.png", hash: "7e3a1c5d9b2f4086a1c3e5d7f9b0a2c4e6d8f0a1b3c5d7e9f0a2b4c6d8e0f1a2", text: null, sources: [], unresolved_placeholders: [], ...over });
}
