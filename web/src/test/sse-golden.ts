// The golden SSE frames of e2e/fixtures/sse-golden as strings, for component tests.
//
// PROVENANCE: those files were written BY HAND, by T1, from the format of crates/hub-platform/src/events/sse.rs
// (retry_frame, heartbeat_frame, event_frame). They were NOT captured from a running hub: the hub's own tests do not dump
// bytes. They are the best statement of the wire format the repository has.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "e2e", "fixtures", "sse-golden");

export const golden = (name: string): string => readFileSync(join(dir, name), "utf8");

export const FIRST_FRAME = golden("first-frame.txt");
export const HEARTBEAT = golden("heartbeat.txt");
export const FILE_OBSERVED_GOLDEN = golden("file-observed.txt");
export const FINDINGS_GOLDEN = golden("findings-changed.txt");
export const RESYNC_GOLDEN = golden("resync.txt");
export const FINAL_RETRY = golden("final-retry.txt");
