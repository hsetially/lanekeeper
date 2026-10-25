import { describe, expect, it } from "vitest";
import { AttributionSource, Confidence, type Attribution } from "@/api/generated/model";
import { CONFIDENCE_LEVELS, SOURCE_LABEL, confidenceView, describeAttribution, evidenceRows, shortUserId, shownConfidence, userLabel } from "./model";

// Who changed a file and how sure the server is (D73), in words. Every statement here comes from the attribution the hub sent: a
// name is only ever the one the caller's lookup gives, a confidence is only ever the one the server gave, and where the server
// names nobody the words say so. Nothing is guessed.

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const OTHER = "11111111-1111-4111-8111-111111111111:3f2a91bc-0000-4000-8000-000000000003";

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME }, evidence: {}, ...over });
const named = (user: string) => ({ kind: "user" as const, user });

describe("the four levels of confidence", () => {
  it("run from low to certain, one quarter each", () => {
    expect(CONFIDENCE_LEVELS).toEqual(["low", "medium", "high", "certain"]);
    expect(CONFIDENCE_LEVELS.map((c) => confidenceView(c))).toEqual([
      { level: "low", label: "Low", step: 1, steps: 4 },
      { level: "medium", label: "Medium", step: 2, steps: 4 },
      { level: "high", label: "High", step: 3, steps: 4 },
      { level: "certain", label: "Certain", step: 4, steps: 4 },
    ]);
  });

  it("covers every level of the contract and no other", () => {
    expect([...CONFIDENCE_LEVELS].sort()).toEqual(Object.values(Confidence).sort());
  });

  it("has a way to say that there is no score, which is not a level", () => {
    expect(confidenceView(null)).toEqual({ level: null, label: "Unknown", step: 0, steps: 4 });
  });
});

describe("the confidence that is shown", () => {
  it("is the server's, for every source that names evidence", () => {
    for (const source of Object.values(AttributionSource).filter((s) => s !== "unknown")) {
      for (const confidence of CONFIDENCE_LEVELS) expect(shownConfidence(attribution({ source, confidence })), `${source} ${confidence}`).toBe(confidence);
    }
  });

  it("is none for an unknown source, whatever the server wrote (the contract carries low there; D73 says none)", () => {
    for (const confidence of CONFIDENCE_LEVELS) expect(shownConfidence(attribution({ source: "unknown", confidence, actor: null }))).toBeNull();
  });

  it("is kept when an NFS client is seen but nobody is named: that is a real finding, with a real level", () => {
    expect(shownConfidence(attribution({ source: "nfs_client", confidence: "medium", actor: null }))).toBe("medium");
  });
});

describe("a user", () => {
  it("is short-named by the first eight characters of the object part of the id", () => {
    expect(shortUserId(OTHER)).toBe("3f2a91bc");
    expect(shortUserId("no-colon-here")).toBe("no-colon");
    expect(shortUserId("")).toBe("");
  });

  it("is You for the signed-in person, and never looked up", () => {
    const calls: string[] = [];
    const nameOf = (id: string) => {
      calls.push(id);
      return "Priya Shah";
    };
    expect(userLabel(ME, { you: ME, nameOf })).toBe("You");
    expect(calls).toEqual([]);
  });

  it("is called by the name the caller's lookup gives", () => {
    expect(userLabel(OTHER, { you: ME, nameOf: (id) => (id === OTHER ? "Marco Diaz" : undefined) })).toBe("Marco Diaz");
  });

  it("is never given a made-up name: with no lookup, or no answer, or a blank one, it is the user's short id", () => {
    expect(userLabel(OTHER, { you: ME })).toBe("Lanekeeper user 3f2a91bc");
    expect(userLabel(OTHER)).toBe("Lanekeeper user 3f2a91bc");
    expect(userLabel(OTHER, { nameOf: () => undefined })).toBe("Lanekeeper user 3f2a91bc");
    expect(userLabel(OTHER, { nameOf: () => "   " })).toBe("Lanekeeper user 3f2a91bc");
    expect(userLabel(OTHER, { nameOf: () => "" })).toBe("Lanekeeper user 3f2a91bc");
  });

  it("is not You when nobody is signed in", () => {
    expect(userLabel(ME, { you: null })).toBe("Lanekeeper user 00000000");
  });
});

describe("describeAttribution: who, how, and how sure", () => {
  it("says You, via Lanekeeper and Certain for the signed-in person's own tool write", () => {
    expect(describeAttribution(attribution(), { you: ME })).toMatchObject({
      who: { kind: "you", label: "You" },
      how: "via Lanekeeper",
      source: "tool_write",
      sourceLabel: "Via Lanekeeper",
      confidence: { level: "certain", label: "Certain", step: 4 },
      known: true,
    });
  });

  it("puts the design's middle dot between the name and the two phrases that continue it, and none before a bracketed note or an empty one", () => {
    const dotted = (over: Partial<Attribution>) => describeAttribution(attribution(over)).dotted;
    expect(dotted({ source: "tool_write" })).toBe(true);
    expect(dotted({ source: "sentinel_login", actor: { kind: "os_login", login: "dana" } })).toBe(true);
    expect(dotted({ source: "nfs_client", actor: null })).toBe(false);
    expect(dotted({ source: "fs_owner_hint", actor: { kind: "os_login", login: "svc" } })).toBe(false);
    expect(dotted({ source: "sync_job", actor: null })).toBe(false);
    expect(dotted({ source: "unknown", actor: null })).toBe(false);
  });

  it("names another person only by the lookup, else by the short id", () => {
    expect(describeAttribution(attribution({ actor: named(OTHER) }), { you: ME, nameOf: () => "Marco Diaz" }).who).toMatchObject({ kind: "user", label: "Marco Diaz" });
    expect(describeAttribution(attribution({ actor: named(OTHER) }), { you: ME }).who).toMatchObject({ kind: "user", label: "Lanekeeper user 3f2a91bc" });
  });

  it("names an OS login by its own text, for an edit on the NFS VM", () => {
    expect(describeAttribution(attribution({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" } }), { you: ME })).toMatchObject({
      who: { kind: "os_login", label: "dana.okoye" },
      how: "edited on the NFS VM",
      confidence: { level: "high", step: 3 },
    });
  });

  it("names a sync job by the job's name; the job's uid is not a person and not a run number", () => {
    const view = describeAttribution(attribution({ source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp-tenant-data-sit7", uid: "7d1c0b5e-1111-4111-8111-000000000042" } } }));
    expect(view.who).toMatchObject({ kind: "job", label: "Sync job csp-tenant-data-sit7" });
    expect(view.how).toBe("");
    expect(view.who.label).not.toContain("7d1c0b5e");
  });

  it("calls an NFS client an NFS client, with the condition that makes it so, and keeps the level the server gave", () => {
    expect(describeAttribution(attribution({ source: "nfs_client", confidence: "medium", actor: null }))).toMatchObject({
      who: { kind: "client", label: "NFS client" },
      how: "(no sync running)",
      confidence: { level: "medium", step: 2 },
      known: true,
    });
  });

  it("says a sync job without naming one", () => {
    expect(describeAttribution(attribution({ source: "sync_job", confidence: "high", actor: null })).who).toMatchObject({ kind: "sync", label: "Sync job" });
  });

  it("says that a file owner is a hint, and that it is low", () => {
    expect(describeAttribution(attribution({ source: "fs_owner_hint", confidence: "low", actor: { kind: "os_login", login: "svc-batch" } }))).toMatchObject({
      who: { label: "svc-batch" },
      how: "(the file's owner, a hint)",
      confidence: { level: "low", step: 1 },
    });
  });

  it("is an unknown author with no score when the source is unknown, and does not say low", () => {
    const view = describeAttribution(attribution({ source: "unknown", confidence: "low", actor: null }));
    expect(view).toMatchObject({ who: { kind: "unknown", label: "Unknown author" }, how: "", confidence: { level: null, label: "Unknown", step: 0 }, known: false });
  });

  it("does not make up a person when the contract names none, for any source", () => {
    for (const source of Object.values(AttributionSource)) {
      const view = describeAttribution(attribution({ source, actor: null }), { you: ME, nameOf: () => "Someone Real" });
      expect(view.who.label, source).not.toContain("Someone Real");
      expect(view.who.label, source).not.toBe("You");
      expect(["client", "sync", "unknown"], source).toContain(view.who.kind);
    }
  });

  it("has a label and a phrase for every source of the contract", () => {
    for (const source of Object.values(AttributionSource)) {
      expect(SOURCE_LABEL[source].length, source).toBeGreaterThan(3);
      expect(describeAttribution(attribution({ source, actor: null })).sourceLabel, source).toBe(SOURCE_LABEL[source]);
    }
    expect(SOURCE_LABEL).toEqual({ tool_write: "Via Lanekeeper", sentinel_login: "Edited on the NFS VM", sync_job: "Sync job", nfs_client: "NFS client", fs_owner_hint: "File owner hint", unknown: "Unknown" });
  });
});

describe("evidenceRows: only what was recorded", () => {
  it("lists the source, the confidence, the author and each piece of evidence that is there, in that order", () => {
    const rows = evidenceRows(
      attribution({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" }, evidence: { executable: "vi", login_identity: "dokoye", observed_at: Date.UTC(2026, 9, 17, 13, 58, 20) } }),
      { you: ME },
    );
    expect(rows).toEqual([
      { label: "Source", value: "Edited on the NFS VM" },
      { label: "Confidence", value: "High" },
      { label: "OS Login user", value: "dana.okoye", mono: true },
      { label: "Executable", value: "vi", mono: true },
      { label: "Login identity", value: "dokoye", mono: true },
      { label: "Observed", value: "2026-10-17 13:58:20 UTC" },
    ]);
  });

  it("leaves out evidence that is null or absent, and says nothing is known about the author when the contract names nobody", () => {
    expect(evidenceRows(attribution({ source: "unknown", confidence: "low", actor: null, evidence: { executable: null, login_identity: null, observed_at: null } }))).toEqual([
      { label: "Source", value: "Unknown" },
      { label: "Confidence", value: "None" },
      { label: "Author", value: "Not identified" },
    ]);
  });

  it("names a user by the same words as the chip, a job by name and uid", () => {
    expect(evidenceRows(attribution({ actor: named(ME) }), { you: ME }).find((r) => r.label === "Author")).toEqual({ label: "Author", value: "You" });
    expect(evidenceRows(attribution({ source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp-tenant-data-sit7", uid: "u-42" } } }))).toEqual(
      expect.arrayContaining([
        { label: "Sync job", value: "csp-tenant-data-sit7" },
        { label: "Job uid", value: "u-42", mono: true },
      ]),
    );
  });

  it("gives a time that cannot be shown as unknown time, not as a date it made up", () => {
    const rows = evidenceRows(attribution({ evidence: { observed_at: -5 } }));
    expect(rows.find((r) => r.label === "Observed")?.value).toBe("unknown time");
  });

  it("hands text through as it is: it is untrusted, and the screen puts it through SafeText", () => {
    const login = "x\u202egnp.exe";
    expect(evidenceRows(attribution({ source: "sentinel_login", actor: { kind: "os_login", login } })).find((r) => r.label === "OS Login user")?.value).toBe(login);
  });
});
