import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { samRequest } from "@/mocks/design/admin/fixtures";
import { installAdminClock, openAdmin } from "@/mocks/design/admin/harness";
import { newWorld, type World } from "@/mocks/design/admin/world";

// Every name, path, login and account on the Admin screens is text somebody typed: a name can carry a right-to-left override that
// makes another name read in its place, or a zero-width character that makes two names look the same. Each section is given such a
// value in every field it shows, and has to draw the character as a visible marker (SafeText), not as itself.

installAdminClock();
const RLO = "‮";
const ZWSP = "​";
const MARKER = /U\+(202E|200B)/;

const CASES: ReadonlyArray<{ section: string; url: string; seed: (world: World) => void; fields: number }> = [
  {
    section: "path mappings",
    url: "/admin/paths",
    seed: (w) => {
      w.paths = [{ id: `rule${ZWSP}one`, base_root: `base${RLO}root`, tenant_root: `tenant${ZWSP}root`, swimlanes: [`lane${ZWSP}a`] }];
    },
    fields: 4,
  },
  {
    section: "merge modes",
    url: "/admin/merge",
    seed: (w) => {
      w.merge = [{ id: `m${ZWSP}1`, file_pattern: `sec${RLO}urity/**`, mode: "whole_file" }];
    },
    fields: 2,
  },
  {
    section: "host tiers",
    url: "/admin/tiers",
    seed: (w) => {
      w.tiers = [{ id: `t${ZWSP}1`, host_pattern: `*${RLO}-dev.example.net`, tier: `d${ZWSP}ev` }];
    },
    fields: 3,
  },
  {
    section: "folder mappings",
    url: "/admin/folders",
    seed: (w) => {
      w.folders = [{ id: `f${ZWSP}1`, folder: `vid${RLO}eo`, deployments: [{ namespace: "sitb", name: `svc${ZWSP}x` }] }];
    },
    fields: 3,
  },
  {
    section: "notification channels",
    url: "/admin/notify",
    seed: (w) => {
      w.channels = [{ id: `c${ZWSP}1`, name: `Ops${RLO}team`, webhook_configured: true, min_severity: "low", kinds: ["drift_detected"], enabled: true }];
    },
    fields: 2,
  },
  {
    section: "agents",
    url: "/admin/swimlanes",
    seed: (w) => {
      w.agents = [{ swimlane: `sit${ZWSP}b`, service_account_email: `agent${RLO}@p.iam.gserviceaccount.com`, status: "connected", connected_replica: `pod${ZWSP}1`, last_seen_at: 1_760_000_000_000, certificate_expires_at: null }];
      w.lanes = w.lanes.map((l, i) => (i === 0 ? { ...l, display_name: `Sit${RLO}one`, project: `proj${ZWSP}x`, tenants: [`t${ZWSP}1`] } : l));
    },
    fields: 6,
  },
  {
    section: "users",
    url: "/admin/users",
    seed: (w) => {
      w.users = w.users.map((u, i) => (i === 2 ? { ...u, display_name: `Len${RLO}a`, email: `l${ZWSP}ena@corp.example`, github_login: `len${ZWSP}a` } : u));
      w.requests = [samRequest({ email: `sam${RLO}@corp.example`, reason: `please${ZWSP}now` })];
    },
    fields: 5,
  },
];

describe("text a person typed", () => {
  it.each(CASES)("is drawn with its hidden characters made visible in $section", async ({ url, seed, fields }) => {
    const world = newWorld();
    seed(world);
    await openAdmin(world, url);
    // Every field the section shows carries its own marker; the lists arrive one after the other, so wait for all of them.
    await waitFor(() => expect(screen.getAllByRole("img", { name: MARKER }).length).toBeGreaterThanOrEqual(fields));
    // And the characters themselves are not in the page.
    expect(document.body.textContent).not.toContain(RLO);
    expect(document.body.textContent).not.toContain(ZWSP);
  });
});
