// The pull-request links of the scenarios of the pull-request pieces (T5, slice B). The data of the Consequence dialogs board,
// frame 1l ("Drift list · PR badges sit beside the drift status and link to the PR"): PR #418 open on the tenant file of sitb, a
// merged one waiting for sync, and the pull request of another swimlane. Times are offsets from the frozen clock the visual runs use
// (1_760_000_000_000). Every hash is the one the swimlanes scenario gives its files, so a badge is not "for an earlier version".
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension and `import type`.
import type { PrLink } from "../../../api/generated/model/index.ts";
import { MARCO, PRIYA } from "../shell/index.ts";

export const NOW = 1_760_000_000_000;
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
/** The hash of the files of the swimlanes scenario. */
export const HASH = "a".repeat(64);

const address = (number: number): string => `https://github.com/csp-tenant/csp-tenant-data/pull/${number}`;

function link(over: Partial<PrLink> & Pick<PrLink, "id" | "path" | "pr_number">): PrLink {
  return {
    swimlane: "sitb",
    nfs_hash: HASH,
    state: "open",
    pr_url: address(over.pr_number),
    author: PRIYA,
    created_at: NOW - 2 * HOUR,
    closed_at: null,
    ...over,
  };
}

/** Newest first, as the hub lists them. #421 carries two files, so it is two links with one number. */
export const LINKS: readonly PrLink[] = [
  link({ id: 14, pr_number: 418, path: "tx-infinity-api/tx-infinity-core-sit7.yml", author: PRIYA, created_at: NOW - 2 * HOUR }),
  link({ id: 13, pr_number: 409, swimlane: "sitc", path: "tx-infinity-api/tx-infinity-gateway.yml", author: MARCO, created_at: NOW - 5 * HOUR }),
  link({ id: 12, pr_number: 421, path: "ui/remote-itm-teller/teller-labels.json", author: MARCO, created_at: NOW - 90 * MIN }),
  link({ id: 11, pr_number: 421, path: "security/security-roles.yml", author: MARCO, created_at: NOW - 90 * MIN }),
  link({ id: 10, pr_number: 417, state: "merged", path: "application.properties", author: MARCO, created_at: NOW - 26 * HOUR, closed_at: NOW - 40 * MIN }),
  link({ id: 9, pr_number: 405, state: "closed", path: "limits/limits.yml", author: PRIYA, created_at: NOW - 4 * DAY, closed_at: NOW - 3 * DAY }),
];

/** What the hub must never be trusted to have sent as a link or a name: the page draws it as text and links none of it. */
export const HOSTILE: readonly PrLink[] = [
  link({ id: 24, pr_number: 11, path: "core/‮txt.yml", pr_url: "javascript:alert(1)" }),
  link({ id: 23, pr_number: 10, path: "core/credentials.yml", pr_url: "https://github.com@evil.example.test/csp/x/pull/10" }),
  link({ id: 22, pr_number: 9, path: "core/plain-http.yml", pr_url: "http://github.com/csp/x/pull/9" }),
  link({ id: 21, pr_number: 8, path: `${"deep/".repeat(120)}config-with-a-very-long-name.yml` }),
];
